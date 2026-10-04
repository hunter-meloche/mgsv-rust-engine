//! Engine-to-script messages and the event timers that produce many of them.
//!
//! The engine delivers messages to the mission's `OnMessage` hook as
//! `OnMessage(scripts, class, message, arg0, arg1, arg2, arg3)`, where `class`
//! and `message` are `StrCode32` ids (e.g. `Timer`/`Finish`) and `arg0` is
//! usually the sender (for timers, `StrCode32` of the timer name). Scripts
//! build dispatch tables keyed the same way (`Tpp.MakeMessageExecTable`).
//!
//! A handler may return `Mission.ON_MESSAGE_RESULT_RESEND` to have the message
//! delivered again on the next frame; `Mission.GetCurrentMessageResendCount`
//! tells it how many times that has happened.

use std::collections::{HashMap, VecDeque};

use mlua::{Lua, MultiValue, Value};

/// Host-defined value for `Mission.ON_MESSAGE_RESULT_RESEND`. Scripts only
/// return it to the engine, so any distinct value works.
// TODO(re): replace with the retail value once the Mission module is mapped.
pub const RESEND: i64 = 1;

#[derive(Clone, Debug)]
pub struct Message {
    pub class: u32,
    pub id: u32,
    pub args: [Value; 4],
    pub resend: u32,
}

#[derive(Default)]
struct Timer {
    ends_at: f64,
}

#[derive(Default)]
pub struct MessageState {
    queue: VecDeque<Message>,
    current_resend: u32,
    timers: HashMap<u32, Timer>,
    paused: bool,
    /// Delivered message count by `class.message`, for diagnostics.
    pub delivered: HashMap<String, u64>,
    /// Posted messages by name (`class/message arg0`), for the event log.
    log: Vec<String>,
}

fn describe(class: &str, id: &str, a0: &Value) -> String {
    match a0 {
        Value::Nil => format!("{class}/{id}"),
        v => format!("{class}/{id} {}", v.to_string().unwrap_or_default()),
    }
}

/// Take the log of posted messages.
pub fn take_log(lua: &Lua) -> Vec<String> {
    std::mem::take(&mut state(lua).log)
}

fn state(lua: &Lua) -> mlua::AppDataRefMut<'_, MessageState> {
    if lua.app_data_ref::<MessageState>().is_none() {
        lua.set_app_data(MessageState::default());
    }
    lua.app_data_mut::<MessageState>().expect("installed above")
}

/// Queue a message for delivery at the next delivery point.
pub fn post(lua: &Lua, class: &str, id: &str, args: [Value; 4]) {
    let entry = describe(class, id, &args[0]);
    let mut st = state(lua);
    st.log.push(entry);
    st.queue.push_back(Message {
        class: fox_hash::str_code32(class.as_bytes()),
        id: fox_hash::str_code32(id.as_bytes()),
        args,
        resend: 0,
    });
}

/// Take every queued message.
pub fn take(lua: &Lua) -> Vec<Message> {
    state(lua).queue.drain(..).collect()
}

/// Requeue a message the handler asked to see again.
pub fn requeue(lua: &Lua, mut m: Message) {
    m.resend += 1;
    state(lua).queue.push_back(m);
}

pub fn set_current_resend(lua: &Lua, n: u32) {
    state(lua).current_resend = n;
}

pub fn note_delivered(lua: &Lua, class: &str) {
    *state(lua).delivered.entry(class.to_string()).or_default() += 1;
}

/// Timer ids: names are hashed with StrCode32; numbers are already ids.
fn timer_id(v: &Value) -> Option<u32> {
    match v {
        Value::String(s) => Some(fox_hash::str_code32(&s.as_bytes())),
        Value::Integer(i) => Some(*i as u32),
        Value::Number(n) => Some(*n as u32),
        _ => None,
    }
}

/// Expire timers at time `now` (seconds), posting `Timer`/`Finish` for each.
pub fn tick_timers(lua: &Lua, now: f64) {
    let mut st = state(lua);
    if st.paused {
        return;
    }
    let mut done: Vec<u32> = st
        .timers
        .iter()
        .filter(|(_, t)| now >= t.ends_at)
        .map(|(id, _)| *id)
        .collect();
    done.sort_unstable();
    for id in done {
        st.timers.remove(&id);
        st.log.push(format!("Timer/Finish {id}"));
        st.queue.push_back(Message {
            class: fox_hash::str_code32(b"Timer"),
            id: fox_hash::str_code32(b"Finish"),
            args: [
                Value::Integer(id as i64),
                Value::Nil,
                Value::Nil,
                Value::Nil,
            ],
            resend: 0,
        });
    }
}

/// Install `GkEventTimerManager` and the message parts of `Mission`.
pub fn install(
    lua: &Lua,
    clock: std::sync::Arc<std::sync::Mutex<crate::Clock>>,
    module: impl Fn(&Lua, &str) -> mlua::Result<mlua::Table>,
) -> mlua::Result<()> {
    state(lua);
    let timers = module(lua, "GkEventTimerManager")?;
    for name in ["Start", "StartRaw"] {
        let clock = clock.clone();
        timers.raw_set(
            name,
            lua.create_function(move |lua, (id, seconds): (Value, Option<f64>)| {
                if let Some(id) = timer_id(&id) {
                    let now = clock.lock().unwrap().seconds;
                    state(lua).timers.insert(
                        id,
                        Timer {
                            ends_at: now + seconds.unwrap_or(0.0),
                        },
                    );
                }
                Ok(())
            })?,
        )?;
    }
    timers.raw_set(
        "Stop",
        lua.create_function(|lua, id: Value| {
            if let Some(id) = timer_id(&id) {
                state(lua).timers.remove(&id);
            }
            Ok(())
        })?,
    )?;
    timers.raw_set(
        "StopAll",
        lua.create_function(|lua, ()| {
            state(lua).timers.clear();
            Ok(())
        })?,
    )?;
    timers.raw_set(
        "IsTimerActive",
        lua.create_function(|lua, id: Value| {
            Ok(timer_id(&id).is_some_and(|id| state(lua).timers.contains_key(&id)))
        })?,
    )?;
    timers.raw_set(
        "SetPaused",
        lua.create_function(|lua, p: MultiValue| {
            state(lua).paused = matches!(p.front(), Some(Value::Boolean(true)));
            Ok(())
        })?,
    )?;

    let mission = module(lua, "Mission")?;
    mission.raw_set("ON_MESSAGE_RESULT_RESEND", RESEND)?;
    mission.raw_set(
        "GetCurrentMessageResendCount",
        lua.create_function(|lua, ()| Ok(state(lua).current_resend))?,
    )?;
    Ok(())
}
