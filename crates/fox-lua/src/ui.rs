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

/// A running screen fade (`FadeFunction.CallFadeIn/CallFadeOut`, engine
/// 0x1405c7c80). The engine keeps one fade object: a new fade replaces the
/// running one. When it finishes it posts `UI`/`EndFadeIn` or `EndFadeOut`
/// with the two values the call stored (scripts pass the fade's name hash as
/// the first, which they match as the message sender).
#[derive(Debug, Clone)]
pub struct Fade {
    pub message: &'static str,
    pub ends_at: f64,
    pub args: [i64; 2],
}

/// `SplashScreen`: timed full-screen images (publisher logos, rating notices).
///
/// `Create(name, texture, ...)` returns a handle; `Show(handle, fade_in, hold,
/// fade_out)` starts it; `GetSplashScreenWithName(name)` returns the handle
/// until it is gone. Scripts may register `SetStateCallback(handle, f)`; the
/// engine calls `f(handle, state)` on state changes, and scripts move on when
/// they see `SplashScreen.STATE_DELETE` (5).
pub struct Splash {
    pub name: String,
    handle: mlua::RegistryKey,
    ends_at: Option<f64>,
    callback: Option<mlua::RegistryKey>,
}

/// Recovered `SplashScreen.STATE_DELETE`.
pub const SPLASH_STATE_DELETE: i64 = 5;

#[derive(Default)]
pub struct UiState {
    pub pre_title: PreTitle,
    pub fade: Option<Fade>,
    pub splashes: Vec<Splash>,
    /// Default fade duration when the call gives none (seconds).
    pub default_fade_seconds: f64,
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

pub fn install(
    lua: &Lua,
    clock: std::sync::Arc<std::sync::Mutex<crate::Clock>>,
    module: impl Fn(&Lua, &str) -> mlua::Result<Table>,
) -> mlua::Result<()> {
    state(lua).default_fade_seconds = 0.5;

    let fade = module(lua, "FadeFunction")?;
    for (name, message) in [("CallFadeIn", "EndFadeIn"), ("CallFadeOut", "EndFadeOut")] {
        let clock = clock.clone();
        fade.raw_set(
            name,
            lua.create_function(
                move |lua, (secs, a0, a1): (Option<f64>, Option<f64>, Option<f64>)| {
                    let now = clock.lock().unwrap().seconds;
                    let mut st = state(lua);
                    let secs = secs.unwrap_or(st.default_fade_seconds);
                    st.fade = Some(Fade {
                        message,
                        ends_at: now + secs.max(0.0),
                        args: [a0.unwrap_or(0.0) as i64, a1.unwrap_or(0.0) as i64],
                    });
                    Ok(())
                },
            )?,
        )?;
    }
    fade.raw_set(
        "IsFadeProcessing",
        lua.create_function(|lua, ()| Ok(state(lua).fade.is_some()))?,
    )?;
    for name in [
        "SetFadeCallEnable",
        "SetFadeColor",
        "ResetFadeColor",
        "InitFadeSetting",
    ] {
        fade.raw_set(name, lua.create_function(|_, _: mlua::MultiValue| Ok(()))?)?;
    }

    // --- SplashScreen ---------------------------------------------------------
    let splash = module(lua, "SplashScreen")?;
    let handle_name = |h: &Table| -> mlua::Result<String> { h.get("name") };
    splash.raw_set(
        "Create",
        lua.create_function(
            |lua, (name, texture, _rest): (String, Option<String>, mlua::MultiValue)| {
                let handle = lua.create_table()?;
                handle.set("name", name.as_str())?;
                handle.set("texture", texture)?;
                let key = lua.create_registry_value(handle.clone())?;
                let mut st = state(lua);
                st.splashes.retain(|s| s.name != name);
                st.splashes.push(Splash {
                    name,
                    handle: key,
                    ends_at: None,
                    callback: None,
                });
                Ok(handle)
            },
        )?,
    )?;
    {
        let clock = clock.clone();
        splash.raw_set(
            "Show",
            lua.create_function(
                move |lua,
                      (h, fade_in, hold, fade_out): (
                    Table,
                    Option<f64>,
                    Option<f64>,
                    Option<f64>,
                )| {
                    let name = handle_name(&h)?;
                    let now = clock.lock().unwrap().seconds;
                    let total =
                        fade_in.unwrap_or(0.0) + hold.unwrap_or(0.0) + fade_out.unwrap_or(0.0);
                    if let Some(s) = state(lua).splashes.iter_mut().find(|s| s.name == name) {
                        s.ends_at = Some(now + total.max(0.0));
                    }
                    Ok(())
                },
            )?,
        )?;
    }
    splash.raw_set(
        "SetStateCallback",
        lua.create_function(move |lua, (h, f): (Table, mlua::Function)| {
            let name = handle_name(&h)?;
            let key = lua.create_registry_value(f)?;
            if let Some(s) = state(lua).splashes.iter_mut().find(|s| s.name == name) {
                s.callback = Some(key);
            }
            Ok(())
        })?,
    )?;
    splash.raw_set(
        "Delete",
        lua.create_function(move |lua, h: Value| {
            if let Value::Table(h) = h {
                let name = handle_name(&h)?;
                state(lua).splashes.retain(|s| s.name != name);
            }
            Ok(())
        })?,
    )?;
    splash.raw_set(
        "GetSplashScreenWithName",
        lua.create_function(|lua, name: String| {
            let key = {
                let st = state(lua);
                match st.splashes.iter().find(|s| s.name == name) {
                    Some(s) => lua.registry_value::<Value>(&s.handle)?,
                    None => Value::Nil,
                }
            };
            Ok(key)
        })?,
    )?;

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

/// Advance native UI flows by one frame (`now` in seconds). Returns a
/// description of anything that happened, for the game's event log.
pub fn tick(lua: &Lua, now: f64) -> mlua::Result<Option<String>> {
    let finished = {
        let mut st = state(lua);
        match &st.fade {
            Some(f) if now >= f.ends_at => st.fade.take(),
            _ => None,
        }
    };
    if let Some(f) = finished {
        crate::messages::post(
            lua,
            "UI",
            f.message,
            [
                Value::Integer(f.args[0]),
                Value::Integer(f.args[1]),
                Value::Nil,
                Value::Nil,
            ],
        );
    }

    // Finished splashes are removed, then their callbacks see STATE_DELETE.
    let done: Vec<Splash> = {
        let mut st = state(lua);
        let (done, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut st.splashes)
            .into_iter()
            .partition(|s| s.ends_at.is_some_and(|t| now >= t));
        st.splashes = keep;
        done
    };
    for s in done {
        if let Some(cb) = &s.callback {
            let f: mlua::Function = lua.registry_value(cb)?;
            let h: Value = lua.registry_value(&s.handle)?;
            f.call::<()>((h, SPLASH_STATE_DELETE))?;
        }
    }

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
