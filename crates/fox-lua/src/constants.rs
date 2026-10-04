//! Native enum constants recovered from the retail binary.
//!
//! `re/api/lua_bindings.json` is produced by `re/Run-GhidraExports.ps1` +
//! `tools/lua_bindings.py pair` from each module's registration code. It holds
//! identifiers and integer values only. Installing them gives scripts the
//! same numbers the original engine exposes (e.g. `TppGameObject.PHASE_ALERT`).

use std::collections::BTreeMap;

use mlua::{Lua, Table};

const BINDINGS_JSON: &str = include_str!("../../../re/api/lua_bindings.json");

#[derive(serde::Deserialize)]
struct Bindings {
    constants: BTreeMap<String, BTreeMap<String, i64>>,
}

/// Install every recovered constant. `module` creates or fetches the module
/// table (unknown members of which stay traced phantoms).
pub fn install(
    lua: &Lua,
    module: impl Fn(&Lua, &str) -> mlua::Result<Table>,
) -> mlua::Result<usize> {
    let b: Bindings = serde_json::from_str(BINDINGS_JSON).map_err(mlua::Error::external)?;
    let mut n = 0;
    for (name, consts) in &b.constants {
        let t = module(lua, name)?;
        for (k, v) in consts {
            t.raw_set(k.as_str(), *v)?;
            n += 1;
        }
    }
    Ok(n)
}
