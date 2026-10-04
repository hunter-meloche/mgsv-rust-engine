//! The native side of mission switching (`Mission.*`).
//!
//! Scripts register two callbacks that map a mission or location code to the
//! packages to load (`SetMissionPackagePathFunc`, `SetLocationPackagePathFunc`),
//! then request a switch with `LoadLocation`/`LoadMission` and `Start`. The
//! host picks the request up between frames, terminates the running mission
//! and loads the new one.

use mlua::{Function, Lua, RegistryKey, Value};

#[derive(Default)]
pub struct MissionManager {
    mission_path_func: Option<RegistryKey>,
    location_path_func: Option<RegistryKey>,
    code_list: Option<RegistryKey>,
    /// Argument of the last `LoadMission` call, until taken.
    load_mission: Option<RegistryKey>,
    /// Argument of the last `LoadLocation` call, until taken.
    load_location: Option<RegistryKey>,
    start_requested: bool,
}

/// A pending mission switch.
pub struct MissionRequest {
    pub mission: Value,
    pub location: Value,
    pub start: bool,
}

fn mgr(lua: &Lua) -> mlua::AppDataRefMut<'_, MissionManager> {
    if lua.app_data_ref::<MissionManager>().is_none() {
        lua.set_app_data(MissionManager::default());
    }
    lua.app_data_mut::<MissionManager>()
        .expect("installed above")
}

fn store(lua: &Lua, v: Value) -> mlua::Result<RegistryKey> {
    lua.create_registry_value(v)
}

pub fn install(
    lua: &Lua,
    module: impl Fn(&Lua, &str) -> mlua::Result<mlua::Table>,
) -> mlua::Result<()> {
    mgr(lua);
    let m = module(lua, "Mission")?;
    m.raw_set(
        "SetMissionPackagePathFunc",
        lua.create_function(|lua, f: Function| {
            let k = store(lua, Value::Function(f))?;
            mgr(lua).mission_path_func = Some(k);
            Ok(())
        })?,
    )?;
    m.raw_set(
        "SetLocationPackagePathFunc",
        lua.create_function(|lua, f: Function| {
            let k = store(lua, Value::Function(f))?;
            mgr(lua).location_path_func = Some(k);
            Ok(())
        })?,
    )?;
    m.raw_set(
        "RegisterMissionCodeList",
        lua.create_function(|lua, v: Value| {
            let k = store(lua, v)?;
            mgr(lua).code_list = Some(k);
            Ok(())
        })?,
    )?;
    m.raw_set(
        "LoadMission",
        lua.create_function(|lua, v: Value| {
            let k = store(lua, v)?;
            mgr(lua).load_mission = Some(k);
            Ok(())
        })?,
    )?;
    m.raw_set(
        "LoadLocation",
        lua.create_function(|lua, v: Value| {
            let k = store(lua, v)?;
            mgr(lua).load_location = Some(k);
            Ok(())
        })?,
    )?;
    m.raw_set(
        "Start",
        lua.create_function(|lua, ()| {
            mgr(lua).start_requested = true;
            Ok(())
        })?,
    )?;
    Ok(())
}

/// Take the pending switch request, if `LoadMission` was called.
pub fn take_request(lua: &Lua) -> mlua::Result<Option<MissionRequest>> {
    let (mission, location, start) = {
        let mut m = mgr(lua);
        let Some(mission) = m.load_mission.take() else {
            return Ok(None);
        };
        (
            mission,
            m.load_location.take(),
            std::mem::take(&mut m.start_requested),
        )
    };
    let req = MissionRequest {
        mission: lua.registry_value(&mission)?,
        location: match &location {
            Some(k) => lua.registry_value(k)?,
            None => Value::Nil,
        },
        start,
    };
    lua.remove_registry_value(mission)?;
    if let Some(k) = location {
        lua.remove_registry_value(k)?;
    }
    Ok(Some(req))
}

/// Call the script-registered package path function for a mission code.
pub fn mission_packages(lua: &Lua, code: Value) -> mlua::Result<Value> {
    let f = {
        let m = mgr(lua);
        match &m.mission_path_func {
            Some(k) => lua.registry_value::<Function>(k)?,
            None => return Ok(Value::Nil),
        }
    };
    f.call(code)
}

/// Call the script-registered package path function for a location code.
pub fn location_packages(lua: &Lua, code: Value) -> mlua::Result<Value> {
    let f = {
        let m = mgr(lua);
        match &m.location_path_func {
            Some(k) => lua.registry_value::<Function>(k)?,
            None => return Ok(Value::Nil),
        }
    };
    f.call(code)
}
