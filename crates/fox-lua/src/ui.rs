//! Native UI flows that call back into scripts.
//!
//! Pre-title sequence (`TppUiCommand.StartPreTitleSequence`, engine
//! 0x1408617e0 / state machine 0x14085c8c0): the engine shows the `PreTitle`
//! layout (logos, notices), then calls the script function
//! `TppVarInit.StartTitle()`, which picks the title mission and requests it
//! with `Mission.LoadMission`. The flow ends when scripts call
//! `TppUiCommand.EndPreTitleSequence`.
//!
//! Without a renderer the logos take no time: the call happens on the frame
//! after the request.

use mlua::{Function, Lua, Table, Value};

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreTitle {
    #[default]
    Idle,
    /// Requested; `TppVarInit.StartTitle` runs on the next tick.
    Requested,
    /// `StartTitle` ran; waiting for `EndPreTitleSequence`.
    WaitingForEnd,
}

#[derive(Default)]
pub struct UiState {
    pub pre_title: PreTitle,
}

fn state(lua: &Lua) -> mlua::AppDataRefMut<'_, UiState> {
    if lua.app_data_ref::<UiState>().is_none() {
        lua.set_app_data(UiState::default());
    }
    lua.app_data_mut::<UiState>().expect("installed above")
}

pub fn pre_title(lua: &Lua) -> PreTitle {
    state(lua).pre_title
}

pub fn install(lua: &Lua, module: impl Fn(&Lua, &str) -> mlua::Result<Table>) -> mlua::Result<()> {
    state(lua);
    let ui = module(lua, "TppUiCommand")?;
    ui.raw_set(
        "StartPreTitleSequence",
        lua.create_function(|lua, ()| {
            state(lua).pre_title = PreTitle::Requested;
            Ok(())
        })?,
    )?;
    ui.raw_set(
        "EndPreTitleSequence",
        lua.create_function(|lua, ()| {
            state(lua).pre_title = PreTitle::Idle;
            Ok(())
        })?,
    )?;
    Ok(())
}

/// Advance native UI flows by one frame. Returns a description of anything
/// that happened, for the game's event log.
pub fn tick(lua: &Lua) -> mlua::Result<Option<String>> {
    if pre_title(lua) != PreTitle::Requested {
        return Ok(None);
    }
    state(lua).pre_title = PreTitle::WaitingForEnd;
    let var_init: Table = lua.globals().get("TppVarInit")?;
    match var_init.get::<Value>("StartTitle")? {
        Value::Function(f) => {
            let f: Function = f;
            f.call::<()>(())?;
            Ok(Some("pre-title finished: TppVarInit.StartTitle()".into()))
        }
        _ => Err(mlua::Error::runtime("TppVarInit.StartTitle is not loaded")),
    }
}
