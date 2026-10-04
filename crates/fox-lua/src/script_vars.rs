//! `TppScriptVars`: typed variable stores behind `gvars` and `svars`.
//!
//! Scripts declare variables with tables such as
//! `{name="x", type=TppScriptVars.TYPE_UINT8, value=0, arraySize=4, save=true, category=...}`
//! and then use them as `gvars.x[0] = 3`. Values are coerced to the declared
//! type; arrays are fixed-size and 0-based.
//!
//! The numeric values of the `TYPE_*`/`CATEGORY_*` constants are placeholders
//! chosen here (scripts only hand them back to the engine). They will be
//! replaced by the retail values once recovered from the executable.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mlua::{Lua, MetaMethod, Table, UserData, UserDataMethods, Value};

use crate::Trace;

/// Array size used when a declaration's size is a native constant we have not
/// recovered yet.
const UNKNOWN_ARRAY_SIZE: i64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarType {
    Bool,
    Int8,
    UInt8,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Float,
}

impl VarType {
    const ALL: [(&'static str, VarType); 10] = [
        ("TYPE_BOOL", VarType::Bool),
        ("TYPE_INT8", VarType::Int8),
        ("TYPE_UINT8", VarType::UInt8),
        ("TYPE_INT16", VarType::Int16),
        ("TYPE_UINT16", VarType::UInt16),
        ("TYPE_INT32", VarType::Int32),
        ("TYPE_UINT32", VarType::UInt32),
        ("TYPE_FLOAT", VarType::Float),
        // Short aliases used by some scripts.
        ("TYPE_INT", VarType::Int32),
        ("TYPE_UINT", VarType::UInt32),
    ];

    fn code(self) -> i64 {
        self as i64
    }

    fn from_code(c: i64) -> Option<Self> {
        Self::ALL.iter().map(|(_, t)| *t).find(|t| t.code() == c)
    }
}

/// Category constants; bit flags so scripts can combine them.
const CATEGORIES: &[&str] = &[
    "CATEGORY_GAME_GLOBAL",
    "CATEGORY_MISSION",
    "CATEGORY_RETRY",
    "CATEGORY_MB_MANAGEMENT",
    "CATEGORY_QUEST",
    "CATEGORY_CONFIG",
    "CATEGORY_PERSONAL",
    "CATEGORY_MISSION_RESTARTABLE",
    "CATEGORY_MGO",
];

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

impl UserData for VarArray {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Index, |_, this, i: i64| {
            let s = this.store.borrow();
            let v = &s.vars[&this.name];
            Ok(usize::try_from(i)
                .ok()
                .and_then(|i| v.values.get(i))
                .map_or(Value::Nil, |x| x.to_lua()))
        });
        m.add_meta_method(MetaMethod::NewIndex, |_, this, (i, val): (i64, Value)| {
            let mut s = this.store.borrow_mut();
            let v = s.vars.get_mut(&this.name).unwrap();
            // Retail scripts write one past the end (TppVarInit loops
            // 0..=MISSION_COUNT_MAX over a MISSION_COUNT_MAX array) and the
            // game does not fault, so out-of-range writes are dropped.
            // TODO(re): confirm against the native store's bounds check.
            if let Some(slot) = usize::try_from(i).ok().and_then(|i| v.values.get_mut(i)) {
                *slot = Scalar::coerce(v.ty, &val)?;
            }
            Ok(())
        });
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
                if let Value::String(k) = &key {
                    let k = k.to_str()?.to_string();
                    if let Some(v) = this.store.borrow_mut().vars.get_mut(&k) {
                        if v.is_array {
                            return Err(mlua::Error::runtime(format!(
                                "cannot assign to array script var {k}"
                            )));
                        }
                        v.values[0] = Scalar::coerce(v.ty, &val)?;
                        return Ok(());
                    }
                }
                lua.registry_value::<Table>(&this.loose)?.raw_set(key, val)
            },
        );
    }
}

impl VarStoreHandle {
    fn new(lua: &Lua) -> mlua::Result<Self> {
        Ok(Self {
            store: Shared::default(),
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
}

pub fn install(lua: &Lua, trace: Arc<Mutex<Trace>>) -> mlua::Result<()> {
    let g = lua.globals();
    let gvars = VarStoreHandle::new(lua)?;
    let svars = VarStoreHandle::new(lua)?;
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
    for (i, name) in CATEGORIES.iter().enumerate() {
        m.raw_set(*name, 1i64 << i)?;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
