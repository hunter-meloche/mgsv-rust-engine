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
                if let Some(slot) = i.and_then(|i| v.values.get_mut(i)) {
                    *slot = Scalar::coerce(v.ty, &val)?;
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
                v.values[0] = Scalar::coerce(v.ty, &val)?;
                Ok(())
            },
        );
    }
}

impl VarStoreHandle {
    fn new(lua: &Lua, trace: &Arc<Mutex<Trace>>) -> mlua::Result<Self> {
        Ok(Self {
            store: Rc::new(RefCell::new(VarStore {
                vars: HashMap::new(),
                trace: trace.clone(),
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
    let result_ok: Value = m.raw_get("RESULT_OK")?;
    m.raw_set(
        "GetLastResult",
        lua.create_function(move |_, ()| Ok(result_ok.clone()))?,
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
