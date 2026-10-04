//! The top-level game loop: boot, then run missions and follow the scripts'
//! mission switch requests.
//!
//! Mirrors what the native mission manager does: the scripts set
//! `vars.missionCode`, call `Mission.LoadMission{...}`, and the engine
//! terminates the running mission, asks the script-registered package path
//! function for the new mission's packages, loads them and runs the new
//! mission's `OnAllocate`/`OnInitialize`.

use mlua::Value;

use crate::FoxLua;
use crate::mission::Mission;
use crate::mission_manager;

/// Mission code the engine starts with (`init`: boot checks, then title).
pub const INIT_MISSION_CODE: i64 = 1;

/// Frames a coroutine hook may take before it is considered stuck.
const HOOK_FRAME_LIMIT: usize = 100_000;

/// Mission switches at the same frame beyond this are treated as a loop.
const MAX_SWITCHES_PER_FRAME: usize = 4;

/// An engine message to deliver at a given frame: stands in for player input
/// on native UI (e.g. `UI`/`PressStart`, `UI`/`TitleMenu` from `GameStart`).
#[derive(Clone, Debug)]
pub struct ScheduledMessage {
    pub frame: u64,
    pub class: String,
    pub message: String,
    /// Sender name; posted as `StrCode32(sender)` in the first argument.
    pub sender: Option<String>,
}

impl std::str::FromStr for ScheduledMessage {
    type Err = String;
    /// `FRAME:CLASS:MESSAGE[:SENDER]`, e.g. `200:UI:TitleMenu:GameStart`.
    fn from_str(s: &str) -> Result<Self, String> {
        let p: Vec<&str> = s.split(':').collect();
        if !(3..=4).contains(&p.len()) {
            return Err(format!("expected FRAME:CLASS:MESSAGE[:SENDER], got {s}"));
        }
        Ok(Self {
            frame: p[0].parse().map_err(|_| format!("bad frame in {s}"))?,
            class: p[1].to_string(),
            message: p[2].to_string(),
            sender: p.get(3).map(|x| x.to_string()),
        })
    }
}

pub struct Game<'a> {
    pub host: &'a FoxLua,
    /// Messages to post at given frames (input stand-ins).
    pub scheduled: Vec<ScheduledMessage>,
    /// Record every posted engine message in `events`.
    pub log_messages: bool,
    pub mission: Option<Mission>,
    pub mission_code: Option<i64>,
    /// Human-readable events (`frame: text`), for tools and tests.
    pub events: Vec<(u64, String)>,
}

impl<'a> Game<'a> {
    pub fn new(host: &'a FoxLua) -> Self {
        Self {
            host,
            scheduled: Vec::new(),
            log_messages: false,
            mission: None,
            mission_code: None,
            events: Vec::new(),
        }
    }

    fn frame_no(&self) -> u64 {
        self.host.clock.lock().unwrap().frame
    }

    fn event(&mut self, text: String) {
        self.drain_host_events();
        let f = self.frame_no();
        self.events.push((f, text));
    }

    /// Move events recorded by engine systems into the game's log.
    fn drain_host_events(&mut self) {
        let f = self.frame_no();
        for e in self.host.take_events() {
            self.events.push((f, e));
        }
        if self.log_messages {
            for m in crate::messages::take_log(&self.host.lua) {
                self.events.push((f, format!("message {m}")));
            }
        } else {
            crate::messages::take_log(&self.host.lua);
        }
    }

    /// Run the boot scripts (`/init.lua`, then `/Tpp/start.lua`, which runs
    /// `start2nd.lua` itself).
    pub fn boot(&mut self) -> mlua::Result<()> {
        for script in ["/init.lua", "/Tpp/start.lua"] {
            let frames = self.host.run_coroutine(script, HOOK_FRAME_LIMIT)?;
            self.event(format!("{script} done ({frames} frames)"));
        }
        Ok(())
    }

    /// Packages for a mission code, from the scripts' registered function.
    pub fn mission_packages(&self, code: i64) -> mlua::Result<Vec<String>> {
        match mission_manager::mission_packages(&self.host.lua, Value::Integer(code))? {
            Value::Table(t) => t.sequence_values::<String>().collect(),
            _ => Ok(Vec::new()),
        }
    }

    /// Terminate the running mission (if any) and load `code`.
    pub fn load_mission_code(&mut self, code: i64) -> mlua::Result<()> {
        if let Some(old) = self.mission.take() {
            old.run_hook(self.host, "OnTerminate", HOOK_FRAME_LIMIT)?;
            self.event(format!(
                "mission {} terminated",
                self.mission_code.unwrap_or(-1)
            ));
        }
        self.host
            .lua
            .load(format!("vars.missionCode = {code}"))
            .exec()?;
        let packages = self.mission_packages(code)?;
        if packages.is_empty() {
            return Err(mlua::Error::runtime(format!(
                "no packages for mission {code}"
            )));
        }
        let m = self.host.load_mission_packages(&packages)?;
        self.event(format!(
            "mission {code} loaded from {}",
            packages.join(", ")
        ));
        let blocks = crate::script_block::snapshot(&self.host.lua);
        if !blocks.is_empty() {
            let names: Vec<&str> = blocks.iter().map(|b| b.name.as_str()).collect();
            self.event(format!(
                "mission {code} script blocks: {}",
                names.join(", ")
            ));
        }
        self.mission_code = Some(code);
        for hook in ["OnAllocate", "OnInitialize"] {
            let frames = m.run_hook(self.host, hook, HOOK_FRAME_LIMIT)?;
            self.event(format!("mission {code} {hook} ({frames} frames)"));
        }
        self.mission = Some(m);
        Ok(())
    }

    /// One frame: mission update, clock advance, then any mission switch.
    pub fn frame(&mut self) -> mlua::Result<()> {
        let now = self.frame_no();
        let (due, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut self.scheduled)
            .into_iter()
            .partition(|m| m.frame <= now);
        self.scheduled = keep;
        for m in due {
            let a0 = m.sender.as_ref().map_or(mlua::Value::Nil, |s| {
                mlua::Value::Integer(fox_hash::str_code32(s.as_bytes()) as i64)
            });
            let nil = mlua::Value::Nil;
            crate::messages::post(
                &self.host.lua,
                &m.class,
                &m.message,
                [a0, nil.clone(), nil.clone(), nil],
            );
            let from = m
                .sender
                .as_ref()
                .map(|s| format!(" from {s}"))
                .unwrap_or_default();
            self.event(format!("posted {}/{}{from}", m.class, m.message));
        }
        if let Some(m) = &self.mission {
            m.update(self.host)?;
        }
        self.host.advance_frame()?;
        self.drain_host_events();
        for _ in 0..MAX_SWITCHES_PER_FRAME {
            let Some(req) = mission_manager::take_request(&self.host.lua)? else {
                return Ok(());
            };
            let code: i64 = self.host.lua.load("return vars.missionCode").eval()?;
            let opts = match &req.mission {
                Value::Table(t) => {
                    let mut o: Vec<String> = t
                        .pairs::<String, Value>()
                        .filter_map(|kv| kv.ok())
                        .map(|(k, v)| format!("{k}={}", v.to_string().unwrap_or_default()))
                        .collect();
                    o.sort();
                    o.join(" ")
                }
                _ => String::new(),
            };
            let call = if req.reload {
                "RequestToReload"
            } else {
                "LoadMission"
            };
            self.event(format!("{call} {code} ({opts})"));
            self.load_mission_code(code)?;
        }
        Err(mlua::Error::runtime("mission switch loop"))
    }
}
