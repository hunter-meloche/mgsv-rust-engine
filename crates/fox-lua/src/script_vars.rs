//! `TppScriptVars`: typed variable stores behind `gvars` and `svars`.
//!
//! Scripts declare variables with tables such as
//! `{name="x", type=TppScriptVars.TYPE_UINT8, value=0, arraySize=4, save=true, category=...}`
//! and then use them as `gvars.x[0] = 3`. Values are coerced to the declared
//! type; arrays are fixed-size and 0-based.
//!
//! `TYPE_*` codes match retail; `CATEGORY_*` and the other constants come from
//! `re/api/lua_bindings.json` via [`crate::constants`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mlua::{Lua, MetaMethod, Table, UserData, UserDataMethods, Value};

use crate::Trace;

/// Array size used when a declaration's size is a native constant we have not
/// recovered yet.
const UNKNOWN_ARRAY_SIZE: i64 = 1024;

/// Variable types, with the retail engine's `TppScriptVars.TYPE_*` codes
/// (recovered from the binding registration in mgsvtpp.exe).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i64)]
pub enum VarType {
    Bool = 0,
    Int32 = 1,
    UInt32 = 2,
    Float = 3,
    Int8 = 4,
    UInt8 = 5,
    Int16 = 6,
    UInt16 = 7,
}

impl VarType {
    const ALL: [(&'static str, VarType); 8] = [
        ("TYPE_BOOL", VarType::Bool),
        ("TYPE_INT32", VarType::Int32),
        ("TYPE_UINT32", VarType::UInt32),
        ("TYPE_FLOAT", VarType::Float),
        ("TYPE_INT8", VarType::Int8),
        ("TYPE_UINT8", VarType::UInt8),
        ("TYPE_INT16", VarType::Int16),
        ("TYPE_UINT16", VarType::UInt16),
    ];

    fn code(self) -> i64 {
        self as i64
    }

    fn from_code(c: i64) -> Option<Self> {
        Self::ALL.iter().map(|(_, t)| *t).find(|t| t.code() == c)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scalar {
    Bool(bool),
    Int(i64),
    Float(f32),
}

impl Scalar {
    fn coerce(ty: VarType, v: &Value) -> mlua::Result<Self> {
        let num = |v: &Value| -> mlua::Result<f64> {
            match v {
                Value::Integer(i) => Ok(*i as f64),
                Value::Number(n) => Ok(*n),
                Value::Boolean(b) => Ok(*b as u8 as f64),
                Value::Nil => Ok(0.0),
                other => Err(mlua::Error::runtime(format!(
                    "cannot store {} in a script var",
                    other.type_name()
                ))),
            }
        };
        Ok(match ty {
            VarType::Bool => Scalar::Bool(match v {
                Value::Boolean(b) => *b,
                Value::Nil => false,
                other => num(other)? != 0.0,
            }),
            VarType::Float => Scalar::Float(num(v)? as f32),
            int => {
                let x = num(v)? as i64;
                Scalar::Int(match int {
                    VarType::Int8 => x as i8 as i64,
                    VarType::UInt8 => x as u8 as i64,
                    VarType::Int16 => x as i16 as i64,
                    VarType::UInt16 => x as u16 as i64,
                    VarType::Int32 => x as i32 as i64,
                    _ => x as u32 as i64,
                })
            }
        })
    }

    fn to_lua(self) -> Value {
        match self {
            Scalar::Bool(b) => Value::Boolean(b),
            Scalar::Int(i) => Value::Integer(i),
            Scalar::Float(f) => Value::Number(f as f64),
        }
    }
}

#[derive(Debug)]
pub struct Var {
    pub ty: VarType,
    pub save: bool,
    pub category: i64,
    default: Scalar,
    values: Vec<Scalar>,
    is_array: bool,
}

#[derive(Default)]
pub struct VarStore {
    pub vars: HashMap<String, Var>,
    trace: Arc<Mutex<Trace>>,
    /// Only `svars` records changes: the engine reports each changed value to
    /// the mission's `OnChangeSVars(name, index)` hook, and `TppSequence`
    /// switches sequences from there (`svars.seq_sequence`).
    record_changes: bool,
    /// `TppScriptVars.SetSVarsNotificationEnabled`.
    notify: bool,
    /// Pending `(name, array index)` change notifications.
    pub changes: Vec<(String, Option<usize>)>,
}

impl VarStore {
    fn note_change(&mut self, name: &str, index: Option<usize>) {
        if self.record_changes && self.notify {
            self.changes.push((name.to_string(), index));
        }
    }
}

/// Prepare a value for storing: phantoms (native constants not recovered
/// yet) become numbers via [`crate::lua_number`], which records them.
fn incoming(lua: &Lua, store: &Shared, val: Value) -> mlua::Result<Value> {
    match val {
        Value::Table(_) => {
            let trace = store.borrow().trace.clone();
            Ok(Value::Number(crate::lua_number(&trace, lua, &val)?))
        }
        v => Ok(v),
    }
}

impl VarStore {
    /// Restore every variable in the given categories (bit mask) to its
    /// declared default.
    pub fn reset(&mut self, category_mask: i64) {
        for v in self.vars.values_mut() {
            if v.category & category_mask != 0 {
                v.values.fill(v.default);
            }
        }
    }
}

type Shared = Rc<RefCell<VarStore>>;

/// The `gvars` / `svars` object seen by scripts.
#[derive(Clone)]
pub struct VarStoreHandle {
    store: Shared,
    /// Values written to undeclared names; kept so scripts keep running.
    loose: Rc<mlua::RegistryKey>,
}

/// One array variable, indexed 0..len.
struct VarArray {
    store: Shared,
    name: String,
}

/// Array index from a script value; phantoms (unrecovered constants) read as
/// 0 and are recorded. Non-integral or negative indices map to None.
fn array_index(lua: &Lua, store: &Shared, i: Value) -> mlua::Result<Option<usize>> {
    let n = match incoming(lua, store, i)? {
        Value::Integer(n) => n as f64,
        Value::Number(n) => n,
        _ => return Ok(None),
    };
    Ok((n >= 0.0 && n.fract() == 0.0).then_some(n as usize))
}

impl UserData for VarArray {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, i: Value| {
            let i = array_index(lua, &this.store, i)?;
            let s = this.store.borrow();
            let v = &s.vars[&this.name];
            Ok(i.and_then(|i| v.values.get(i))
                .map_or(Value::Nil, |x| x.to_lua()))
        });
        m.add_meta_method(
            MetaMethod::NewIndex,
            |lua, this, (i, val): (Value, Value)| {
                let i = array_index(lua, &this.store, i)?;
                let val = incoming(lua, &this.store, val)?;
                let mut s = this.store.borrow_mut();
                let v = s.vars.get_mut(&this.name).unwrap();
                // Retail scripts write one past the end (TppVarInit loops
                // 0..=MISSION_COUNT_MAX over a MISSION_COUNT_MAX array) and the
                // game does not fault, so out-of-range writes are dropped.
                // TODO(re): confirm against the native store's bounds check.
                let mut changed = None;
                if let Some((idx, slot)) = i.and_then(|i| v.values.get_mut(i).map(|s| (i, s))) {
                    let new = Scalar::coerce(v.ty, &val)?;
                    if *slot != new {
                        *slot = new;
                        changed = Some(idx);
                    }
                }
                if let Some(idx) = changed {
                    s.note_change(&this.name, Some(idx));
                }
                Ok(())
            },
        );
        m.add_meta_method(MetaMethod::Len, |_, this, ()| {
            Ok(this.store.borrow().vars[&this.name].values.len())
        });
    }
}

impl UserData for VarStoreHandle {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            let Value::String(k) = &key else {
                return Ok(Value::Nil);
            };
            let k = k.to_str()?.to_string();
            let s = this.store.borrow();
            match s.vars.get(&k) {
                Some(v) if v.is_array => Ok(Value::UserData(lua.create_userdata(VarArray {
                    store: this.store.clone(),
                    name: k,
                })?)),
                Some(v) => Ok(v.values[0].to_lua()),
                None => lua.registry_value::<Table>(&this.loose)?.raw_get(key),
            }
        });
        m.add_meta_method(
            MetaMethod::NewIndex,
            |lua, this, (key, val): (Value, Value)| {
                let declared = match &key {
                    Value::String(k) => Some(k.to_str()?.to_string())
                        .filter(|k| this.store.borrow().vars.contains_key(k)),
                    _ => None,
                };
                let Some(k) = declared else {
                    return lua.registry_value::<Table>(&this.loose)?.raw_set(key, val);
                };
                let val = incoming(lua, &this.store, val)?;
                let mut s = this.store.borrow_mut();
                let v = s.vars.get_mut(&k).expect("checked above");
                if v.is_array {
                    return Err(mlua::Error::runtime(format!(
                        "cannot assign to array script var {k}"
                    )));
                }
                let new = Scalar::coerce(v.ty, &val)?;
                if v.values[0] != new {
                    v.values[0] = new;
                    s.note_change(&k, None);
                }
                Ok(())
            },
        );
    }
}

/// The `svars` store, kept in Lua app data so the mission loop can deliver
/// change notifications.
struct SvarsStore(Shared);

/// Take the pending `svars` change notifications (`(name, array index)`).
pub fn take_svars_changes(lua: &Lua) -> Vec<(String, Option<usize>)> {
    lua.app_data_ref::<SvarsStore>()
        .map(|s| std::mem::take(&mut s.0.borrow_mut().changes))
        .unwrap_or_default()
}

impl VarStoreHandle {
    fn new(lua: &Lua, trace: &Arc<Mutex<Trace>>) -> mlua::Result<Self> {
        Ok(Self {
            store: Rc::new(RefCell::new(VarStore {
                vars: HashMap::new(),
                trace: trace.clone(),
                record_changes: false,
                notify: true,
                changes: Vec::new(),
            })),
            loose: Rc::new(lua.create_registry_value(lua.create_table()?)?),
        })
    }

    /// Apply a declaration list (`{ {name=..., type=..., value=..., arraySize=...}, ... }`).
    fn declare(&self, lua: &Lua, trace: &Mutex<Trace>, list: Table) -> mlua::Result<()> {
        let mut s = self.store.borrow_mut();
        for decl in list.sequence_values::<Table>() {
            let decl = decl?;
            let name: String = decl.get("name")?;
            // Numeric fields may be unrecovered native constants (phantoms);
            // lua_number reads those as 0 and records them.
            let field = |f: &str| -> mlua::Result<Option<i64>> {
                match decl.get::<Value>(f)? {
                    Value::Nil => Ok(None),
                    v => crate::lua_number(trace, lua, &v)
                        .map(|n| Some(n as i64))
                        .map_err(|e| mlua::Error::runtime(format!("script var {name}.{f}: {e}"))),
                }
            };
            let ty_code: i64 = field("type")?.unwrap_or(VarType::UInt32.code());
            let ty = VarType::from_code(ty_code).ok_or_else(|| {
                mlua::Error::runtime(format!("script var {name}: unknown type {ty_code}"))
            })?;
            let default = Scalar::coerce(ty, &decl.get::<Value>("value")?)?;
            let array_size = match field("arraySize")? {
                Some(0)
                    if !matches!(
                        decl.get::<Value>("arraySize")?,
                        Value::Integer(0) | Value::Number(0.0)
                    ) =>
                {
                    Some(UNKNOWN_ARRAY_SIZE)
                }
                other => other,
            };
            let len = array_size.unwrap_or(1).max(1) as usize;
            s.vars.insert(
                name.clone(),
                Var {
                    ty,
                    save: decl.get::<Option<bool>>("save")?.unwrap_or(false),
                    category: field("category")?.unwrap_or(0),
                    default,
                    values: vec![default; len],
                    is_array: array_size.is_some_and(|n| n > 1),
                },
            );
        }
        Ok(())
    }

    /// Declare the engine-owned variables (`vars`) from the layout recovered
    /// from the binary. Values start zeroed, as the engine's block does.
    fn declare_layout(&self, layout: &[EngineVar]) {
        let mut s = self.store.borrow_mut();
        for v in layout {
            let Some(ty) = VarType::from_name(&v.ty) else {
                continue;
            };
            let zero = Scalar::coerce(ty, &Value::Nil).expect("nil coerces");
            let len = v.array_size.max(1) as usize;
            s.vars.insert(
                v.name.clone(),
                Var {
                    ty,
                    save: false,
                    category: v.flags,
                    default: zero,
                    values: vec![zero; len],
                    is_array: v.array_size > 1,
                },
            );
        }
    }
}

/// One entry of `re/api/vars_layout.json`.
#[derive(serde::Deserialize, Debug)]
pub struct EngineVar {
    pub name: String,
    pub offset: u32,
    #[serde(rename = "type")]
    pub ty: String,
    pub array_size: u32,
    pub flags: i64,
    pub str_code32: u32,
}

#[derive(serde::Deserialize)]
struct EngineVarLayout {
    vars: Vec<EngineVar>,
}

const VARS_LAYOUT_JSON: &str = include_str!("../../../re/api/vars_layout.json");

/// The engine's `vars` declarations, recovered from mgsvtpp.exe.
pub fn engine_vars_layout() -> Vec<EngineVar> {
    serde_json::from_str::<EngineVarLayout>(VARS_LAYOUT_JSON)
        .expect("re/api/vars_layout.json is valid")
        .vars
}

impl VarType {
    fn from_name(n: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(k, _)| k.strip_prefix("TYPE_") == Some(n))
            .map(|(_, t)| *t)
    }

    pub fn size(self) -> u32 {
        match self {
            VarType::Bool | VarType::Int8 | VarType::UInt8 => 1,
            VarType::Int16 | VarType::UInt16 => 2,
            _ => 4,
        }
    }
}

pub fn install(lua: &Lua, trace: Arc<Mutex<Trace>>) -> mlua::Result<()> {
    let g = lua.globals();
    let gvars = VarStoreHandle::new(lua, &trace)?;
    let svars = VarStoreHandle::new(lua, &trace)?;
    svars.store.borrow_mut().record_changes = true;
    lua.set_app_data(SvarsStore(svars.store.clone()));
    let vars = VarStoreHandle::new(lua, &trace)?;
    vars.declare_layout(&engine_vars_layout());
    g.raw_set("vars", lua.create_userdata(vars)?)?;
    g.raw_set("gvars", lua.create_userdata(gvars.clone())?)?;
    g.raw_set("svars", lua.create_userdata(svars.clone())?)?;

    // Members we implement live in a plain table whose metatable falls back
    // to the phantom module the prelude would otherwise create.
    let m: Table = match g.raw_get::<Value>("TppScriptVars")? {
        Value::Table(t) => t,
        _ => {
            let t = lua.create_table()?;
            let mt = lua.create_table()?;
            let phantom: mlua::Function = g.get("__fox_phantom")?;
            mt.set(
                "__index",
                lua.create_function(move |_, (_t, k): (Table, String)| {
                    phantom.call::<Value>(format!("TppScriptVars.{k}"))
                })?,
            )?;
            t.set_metatable(Some(mt))?;
            g.raw_set("TppScriptVars", t.clone())?;
            t
        }
    };
    for (name, ty) in VarType::ALL {
        m.raw_set(name, ty.code())?;
    }
    let t = trace.clone();
    m.raw_set(
        "DeclareGVars",
        lua.create_function(move |lua, list: Table| gvars.declare(lua, &t, list))?,
    )?;
    m.raw_set(
        "DeclareSVars",
        lua.create_function(move |lua, list: Table| svars.declare(lua, &trace, list))?,
    )?;

    // Save/load state. No save I/O is implemented yet, so nothing is ever in
    // flight and the last operation always succeeded.
    m.raw_set("IsSavingOrLoading", lua.create_function(|_, ()| Ok(false))?)?;
    m.raw_set(
        "SetSVarsNotificationEnabled",
        lua.create_function(|lua, on: bool| {
            if let Some(s) = lua.app_data_ref::<SvarsStore>() {
                s.0.borrow_mut().notify = on;
            }
            Ok(())
        })?,
    )?;
    let result_ok: Value = m.raw_get("RESULT_OK")?;
    m.raw_set(
        "GetLastResult",
        lua.create_function(move |_, ()| Ok(result_ok.clone()))?,
    )?;

    let store_of_global = |name: &str| -> mlua::Result<Shared> {
        Ok(lua
            .globals()
            .get::<mlua::AnyUserData>(name)?
            .borrow::<VarStoreHandle>()?
            .store
            .clone())
    };
    lua.set_app_data(Stores {
        vars: store_of_global("vars")?,
        svars: store_of_global("svars")?,
        gvars: store_of_global("gvars")?,
    });
    install_slots(lua, &m)?;
    Ok(())
}

// --- Save slots ---------------------------------------------------------------
//
// `TppScriptVars` keeps numbered save slots (TppDefine.SAVE_SLOT.*) holding
// copies of variables; files are written from and read into them. Scripts
// use them to carry state across mission loads: the title menu writes the
// prologue's start sequence into the CHECK_POINT slot
// (`SetVarValueInSlot(slot, "svars", "seq_sequence", n)`), and mission start
// restores svars from it with `LoadVarsFromSlot`.
//
// Group bits select stores: GROUP_BIT_VARS (1) = vars, GROUP_BIT_GVARS (4) =
// gvars, and 8 (part of GROUP_BIT_ALL = 15) is taken to be svars.
// Category masks filter by each variable's declared category.
// TODO(re): confirm the svars group bit and the role of bit 2 (cvars).
// Slots live in memory only; writing them to files is not implemented yet.

struct Stores {
    vars: Shared,
    svars: Shared,
    gvars: Shared,
}

type SlotKey = (&'static str, String);

#[derive(Default)]
struct Slots {
    slots: HashMap<i64, HashMap<SlotKey, Vec<Scalar>>>,
}

const STORE_GROUPS: [(&str, i64); 3] = [("vars", 1), ("gvars", 4), ("svars", 8)];

fn store_name(name: &str) -> Option<&'static str> {
    STORE_GROUPS.iter().map(|(n, _)| *n).find(|n| *n == name)
}

fn store_of(lua: &Lua, name: &str) -> Option<Shared> {
    let s = lua.app_data_ref::<Stores>()?;
    Some(match name {
        "vars" => s.vars.clone(),
        "gvars" => s.gvars.clone(),
        "svars" => s.svars.clone(),
        _ => return None,
    })
}

fn slots(lua: &Lua) -> mlua::AppDataRefMut<'_, Slots> {
    if lua.app_data_ref::<Slots>().is_none() {
        lua.set_app_data(Slots::default());
    }
    lua.app_data_mut::<Slots>().expect("installed above")
}

impl Scalar {
    /// Best-effort scalar for a value written to a slot without a declared type.
    fn infer(v: &Value) -> Option<Self> {
        Some(match v {
            Value::Boolean(b) => Scalar::Bool(*b),
            Value::Integer(i) => Scalar::Int(*i),
            Value::Number(n) if n.fract() == 0.0 => Scalar::Int(*n as i64),
            Value::Number(n) => Scalar::Float(*n as f32),
            _ => return None,
        })
    }
}

/// A slot id, or the first element of a `{slot, ...}` table (CopySlot form).
fn slot_id(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(*i),
        Value::Number(n) => Some(*n as i64),
        Value::Table(t) => t.raw_get::<Option<i64>>(1).ok().flatten(),
        _ => None,
    }
}

fn install_slots(lua: &Lua, m: &Table) -> mlua::Result<()> {
    slots(lua);
    m.raw_set(
        "SetVarValueInSlot",
        lua.create_function(
            |lua, (slot, store, name, a, b): (Value, String, String, Value, Value)| {
                let (Some(slot), Some(store)) = (slot_id(&slot), store_name(&store)) else {
                    return Ok(());
                };
                // (slot, store, name, value) or (slot, store, name, index, value)
                let (index, value) = match b {
                    Value::Nil => (0usize, a),
                    v => (
                        match a {
                            Value::Integer(i) => i.max(0) as usize,
                            Value::Number(n) => n.max(0.0) as usize,
                            _ => 0,
                        },
                        v,
                    ),
                };
                let declared = store_of(lua, store)
                    .and_then(|s| s.borrow().vars.get(&name).map(|v| (v.ty, v.values.len())));
                let scalar = match declared {
                    Some((ty, _)) => Scalar::coerce(ty, &value)?,
                    None => match Scalar::infer(&value) {
                        Some(s) => s,
                        None => return Ok(()),
                    },
                };
                let len = declared.map_or(index + 1, |(_, n)| n.max(index + 1));
                let mut sl = slots(lua);
                let entry = sl
                    .slots
                    .entry(slot)
                    .or_default()
                    .entry((store, name))
                    .or_insert_with(Vec::new);
                if entry.len() < len {
                    entry.resize(len, scalar);
                }
                entry[index] = scalar;
                Ok(())
            },
        )?,
    )?;
    m.raw_set(
        "GetVarValueInSlot",
        lua.create_function(
            |lua, (slot, store, name, index): (Value, String, String, Option<i64>)| {
                let (Some(slot), Some(store)) = (slot_id(&slot), store_name(&store)) else {
                    return Ok(Value::Nil);
                };
                let sl = slots(lua);
                Ok(sl
                    .slots
                    .get(&slot)
                    .and_then(|s| s.get(&(store, name)))
                    .and_then(|v| v.get(index.unwrap_or(0).max(0) as usize))
                    .map_or(Value::Nil, |x| x.to_lua()))
            },
        )?,
    )?;
    m.raw_set(
        "SaveVarsToSlot",
        lua.create_function(
            |lua,
             (slot, groups, categories, _rest): (
                Value,
                Option<i64>,
                Option<i64>,
                mlua::MultiValue,
            )| {
                let Some(slot) = slot_id(&slot) else {
                    return Ok(());
                };
                let (groups, categories) = (groups.unwrap_or(15), categories.unwrap_or(-1));
                for (name, bit) in STORE_GROUPS {
                    if groups & bit == 0 {
                        continue;
                    }
                    let Some(store) = store_of(lua, name) else {
                        continue;
                    };
                    let copied: Vec<(String, Vec<Scalar>)> = store
                        .borrow()
                        .vars
                        .iter()
                        .filter(|(_, v)| v.category & categories != 0)
                        .map(|(k, v)| (k.clone(), v.values.clone()))
                        .collect();
                    let mut sl = slots(lua);
                    let s = sl.slots.entry(slot).or_default();
                    for (k, v) in copied {
                        s.insert((name, k), v);
                    }
                }
                Ok(())
            },
        )?,
    )?;
    m.raw_set(
        "LoadVarsFromSlot",
        lua.create_function(
            |lua, (slot, groups, categories): (Value, Option<i64>, Option<i64>)| {
                let Some(slot) = slot_id(&slot) else {
                    return Ok(());
                };
                let (groups, categories) = (groups.unwrap_or(15), categories.unwrap_or(-1));
                let entries: Vec<(SlotKey, Vec<Scalar>)> = slots(lua)
                    .slots
                    .get(&slot)
                    .map(|s| s.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                for ((name, var), values) in entries {
                    let bit = STORE_GROUPS
                        .iter()
                        .find(|(n, _)| *n == name)
                        .map_or(0, |(_, b)| *b);
                    if groups & bit == 0 {
                        continue;
                    }
                    let Some(store) = store_of(lua, name) else {
                        continue;
                    };
                    let mut st = store.borrow_mut();
                    if let Some(live) = st.vars.get_mut(&var) {
                        if live.category & categories == 0 {
                            continue;
                        }
                        for (dst, src) in live.values.iter_mut().zip(&values) {
                            *dst = Scalar::coerce(live.ty, &src.to_lua())?;
                        }
                    }
                }
                Ok(())
            },
        )?,
    )?;
    m.raw_set(
        "CopySlot",
        lua.create_function(|lua, (dst, src): (Value, Value)| {
            let (Some(dst), Some(src)) = (slot_id(&dst), slot_id(&src)) else {
                return Ok(());
            };
            let mut sl = slots(lua);
            let copy = sl.slots.get(&src).cloned().unwrap_or_default();
            sl.slots.insert(dst, copy);
            Ok(())
        })?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binary stores StrCode32(name) next to each name; matching it
    /// validates both the layout decoder and our CityHash/StrCode port.
    #[test]
    fn engine_vars_hashes_match() {
        let layout = engine_vars_layout();
        assert!(layout.len() > 300);
        for v in &layout {
            assert_eq!(
                fox_hash::str_code32(v.name.as_bytes()),
                v.str_code32,
                "{}",
                v.name
            );
        }
    }

    /// Offsets, types and array sizes must describe non-overlapping storage.
    #[test]
    fn engine_vars_do_not_overlap() {
        let mut spans: Vec<(u32, u32, &str)> = engine_vars_layout()
            .iter()
            .map(|v| {
                let ty = VarType::from_name(&v.ty).unwrap_or_else(|| panic!("type {}", v.ty));
                (
                    v.offset,
                    v.offset + ty.size() * v.array_size.max(1),
                    v.name.as_str(),
                )
            })
            .map(|(a, b, n)| (a, b, Box::leak(n.to_string().into_boxed_str()) as &str))
            .collect();
        spans.sort();
        for w in spans.windows(2) {
            assert!(
                w[0].1 <= w[1].0,
                "{} [{:#x},{:#x}) overlaps {} at {:#x}",
                w[0].2,
                w[0].0,
                w[0].1,
                w[1].2,
                w[1].0
            );
        }
    }

    #[test]
    fn declare_and_use() {
        let lua = Lua::new();
        lua.globals()
            .set(
                "__fox_phantom",
                lua.create_function(|_, _: String| Ok(())).unwrap(),
            )
            .unwrap();
        install(&lua, Default::default()).unwrap();
        lua.load(
            r#"
            TppScriptVars.DeclareGVars{
              {name="flag", type=TppScriptVars.TYPE_BOOL, value=false, arraySize=3},
              {name="count", type=TppScriptVars.TYPE_UINT8, value=7},
            }
            assert(gvars.flag[0] == false and #gvars.flag == 3)
            gvars.flag[2] = true
            assert(gvars.flag[2] == true and gvars.flag[3] == nil)
            gvars.flag[3] = true -- out of range: ignored, like retail
            assert(gvars.flag[3] == nil)
            assert(gvars.count == 7)
            gvars.count = 300 -- wraps to uint8
            assert(gvars.count == 44)
            gvars.undeclared = "kept"
            assert(gvars.undeclared == "kept")
            "#,
        )
        .exec()
        .unwrap();
    }
}
