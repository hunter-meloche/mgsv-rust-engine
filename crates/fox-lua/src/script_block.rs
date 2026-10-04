//! Script blocks: named streaming slots for demo, quest, NPC and animal
//! scripts.
//!
//! Blocks are declared by `ScriptBlockData` entities in the loaded mission's
//! DataSets (`name`, `enabled`, `sizeInBytes`, `prerequisites`). Scripts look
//! them up by name, ask for a package to be loaded into one, then activate it.
//!
//! States (recovered constants): EMPTY 0, PROCESSING 1, INACTIVE 2, ACTIVE 3.
//! `Load` moves a block to PROCESSING; on the next frame the package's
//! scripts run and the block becomes INACTIVE (ACTIVE if `Activate` arrived
//! while it loaded). `Activate`/`Deactivate` switch
//! between INACTIVE and ACTIVE.

use std::sync::Arc;

use fox_archive::{Fpk, Vfs};
use mlua::{Lua, MultiValue, Table, Value};

use crate::mission::{data_package_path, sub_script_load_order};

pub const STATE_EMPTY: i64 = 0;
pub const STATE_PROCESSING: i64 = 1;
pub const STATE_INACTIVE: i64 = 2;
pub const STATE_ACTIVE: i64 = 3;
/// Host-defined; scripts only compare ids against it.
// TODO(re): retail value of SCRIPT_BLOCK_ID_INVALID not recovered yet.
pub const INVALID: i64 = 0xFFFF;

#[derive(Debug, Clone)]
pub struct Block {
    pub name: String,
    pub size: u32,
    pub prerequisites: Vec<String>,
    pub state: i64,
    /// Packages requested with `Load` (a path or a list of paths).
    pub packages: Vec<String>,
    /// `Activate` arrived while the block was still loading; it becomes
    /// ACTIVE as soon as the load completes (`TppScriptBlock.Load` activates
    /// right after requesting the load).
    pub activate_on_load: bool,
}

#[derive(Default)]
pub struct ScriptBlocks {
    pub blocks: Vec<Block>,
}

fn blocks(lua: &Lua) -> mlua::AppDataRefMut<'_, ScriptBlocks> {
    if lua.app_data_ref::<ScriptBlocks>().is_none() {
        lua.set_app_data(ScriptBlocks::default());
    }
    lua.app_data_mut::<ScriptBlocks>().expect("installed above")
}

/// Replace the declared blocks (called when a mission loads).
pub fn declare(lua: &Lua, declared: Vec<Block>) {
    blocks(lua).blocks = declared;
}

pub fn snapshot(lua: &Lua) -> Vec<Block> {
    blocks(lua).blocks.clone()
}

fn id_of(lua: &Lua, v: &Value) -> Option<usize> {
    let n = match v {
        Value::Integer(i) => *i,
        Value::Number(f) => *f as i64,
        _ => return None,
    };
    (n >= 0 && (n as usize) < blocks(lua).blocks.len()).then_some(n as usize)
}

pub fn install(lua: &Lua, module: impl Fn(&Lua, &str) -> mlua::Result<Table>) -> mlua::Result<()> {
    blocks(lua);
    let sb = module(lua, "ScriptBlock")?;
    sb.raw_set("SCRIPT_BLOCK_ID_INVALID", INVALID)?;
    sb.raw_set(
        "GetScriptBlockId",
        lua.create_function(|lua, name: Value| {
            let id = match &name {
                Value::String(s) => {
                    let s = s.to_str()?.to_string();
                    blocks(lua).blocks.iter().position(|b| b.name == s)
                }
                // Some callers pass StrCode32 ids.
                Value::Integer(_) | Value::Number(_) => {
                    let h = match name {
                        Value::Integer(i) => i as u32,
                        Value::Number(f) => f as u32,
                        _ => unreachable!(),
                    };
                    blocks(lua)
                        .blocks
                        .iter()
                        .position(|b| fox_hash::str_code32(b.name.as_bytes()) == h)
                }
                _ => None,
            };
            Ok(id.map_or(INVALID, |i| i as i64))
        })?,
    )?;
    sb.raw_set(
        "GetScriptBlockState",
        lua.create_function(|lua, id: Value| {
            Ok(id_of(lua, &id).map_or(STATE_EMPTY, |i| blocks(lua).blocks[i].state))
        })?,
    )?;
    sb.raw_set(
        "Load",
        lua.create_function(|lua, (id, packages): (Value, Value)| {
            let Some(i) = id_of(lua, &id) else {
                return Ok(false);
            };
            let list: Vec<String> = match packages {
                Value::String(s) => vec![s.to_str()?.to_string()],
                Value::Table(t) => t.sequence_values::<String>().collect::<mlua::Result<_>>()?,
                _ => Vec::new(),
            };
            let list: Vec<String> = list.into_iter().filter(|p| !p.is_empty()).collect();
            let mut b = blocks(lua);
            let block = &mut b.blocks[i];
            // Loading nothing empties the block.
            block.state = if list.is_empty() {
                STATE_EMPTY
            } else {
                STATE_PROCESSING
            };
            block.packages = list;
            block.activate_on_load = false;
            Ok(true)
        })?,
    )?;
    sb.raw_set(
        "Activate",
        lua.create_function(|lua, id: Value| {
            let Some(i) = id_of(lua, &id) else {
                return Ok(false);
            };
            let mut b = blocks(lua);
            let block = &mut b.blocks[i];
            match block.state {
                STATE_INACTIVE => block.state = STATE_ACTIVE,
                STATE_PROCESSING => block.activate_on_load = true,
                _ => {}
            }
            Ok(true)
        })?,
    )?;
    sb.raw_set(
        "Deactivate",
        lua.create_function(|lua, id: Value| {
            let Some(i) = id_of(lua, &id) else {
                return Ok(false);
            };
            let mut b = blocks(lua);
            let block = &mut b.blocks[i];
            block.activate_on_load = false;
            if block.state == STATE_ACTIVE {
                block.state = STATE_INACTIVE;
            }
            Ok(true)
        })?,
    )?;
    sb.raw_set(
        "GetCurrentScriptBlockId",
        lua.create_function(|_, _: MultiValue| Ok(INVALID))?,
    )?;
    sb.raw_set(
        "UpdateScriptsInScriptBlocks",
        lua.create_function(|_, _: MultiValue| Ok(()))?,
    )?;
    Ok(())
}

/// Finish pending loads: run each requested package's scripts as libraries
/// and mark the block INACTIVE. Returns `(block, package)` for each load.
pub fn tick(lua: &Lua, vfs: &Arc<Vfs>) -> mlua::Result<Vec<(String, String)>> {
    let pending: Vec<(usize, String, Vec<String>)> = blocks(lua)
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.state == STATE_PROCESSING)
        .map(|(i, b)| (i, b.name.clone(), b.packages.clone()))
        .collect();
    let mut done = Vec::new();
    for (i, name, packages) in pending {
        for package in &packages {
            let data_path = data_package_path(package);
            if let Ok(bytes) = vfs.read(&data_path) {
                if let Ok(pkg) = Fpk::parse(&bytes) {
                    let scripts: Vec<(String, String)> = pkg
                        .entries
                        .iter()
                        .map(|e| e.clean_path())
                        .filter(|p| p.ends_with(".lua"))
                        .map(|p| (String::new(), p))
                        .collect();
                    let read = |p: &str| {
                        pkg.entries
                            .iter()
                            .find(|e| e.clean_path() == p)
                            .and_then(|e| pkg.read(e).ok())
                    };
                    for k in sub_script_load_order(&scripts, read) {
                        let path = &scripts[k].1;
                        if let Some(src) = read(path) {
                            let v: Value =
                                lua.load(&src[..]).set_name(format!("@{path}")).eval()?;
                            if let Value::Table(t) = v {
                                let stem = path
                                    .rsplit('/')
                                    .next()
                                    .unwrap_or(path)
                                    .split('.')
                                    .next()
                                    .unwrap_or(path);
                                lua.globals().raw_set(stem, t)?;
                            }
                        }
                    }
                }
            }
        }
        // A package that is missing still completes, like an empty load.
        {
            let mut b = blocks(lua);
            let block = &mut b.blocks[i];
            block.state = if block.activate_on_load {
                STATE_ACTIVE
            } else {
                STATE_INACTIVE
            };
            block.activate_on_load = false;
        }
        done.push((name, packages.join(", ")));
    }
    Ok(done)
}
