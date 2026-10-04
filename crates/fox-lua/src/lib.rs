//! Lua host for the game's own scripts.
//!
//! The original engine embeds Lua 5.1.5 (+ LuaBitOp); mlua's `lua51` feature
//! builds the same interpreter, so the shipped scripts run unmodified. Native
//! modules are provided by Rust in [`natives`]; anything not yet implemented is
//! a traced "phantom" (see `prelude.lua`).

pub mod constants;
pub mod game;
pub mod messages;
pub mod mission;
pub mod mission_manager;
mod natives;
pub mod script_block;
pub mod script_vars;
pub mod ui;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use fox_archive::Vfs;
use mlua::{Function, Lua, Value};

pub use mlua;

#[derive(Default, Debug)]
pub struct Trace {
    /// Calls to unimplemented natives, by dotted path.
    pub phantom_calls: BTreeMap<String, u64>,
    /// Calls to Rust-implemented natives, by name.
    pub native_calls: BTreeMap<String, u64>,
    /// Unrecovered native constants that scripts used as numbers (read as 0).
    pub phantom_values: BTreeMap<String, u64>,
    /// Scripts loaded, in order.
    pub loaded: Vec<String>,
    /// Script paths requested but not found in the archives.
    pub missing: Vec<String>,
    /// Fox.Log / Warning / Error output.
    pub log: Vec<String>,
}

/// Simulated game time shared with natives that depend on it.
#[derive(Default, Debug)]
pub struct Clock {
    pub frame: u64,
    pub seconds: f64,
}

/// Fixed step used while the host drives scripts without a renderer.
pub const FRAME_SECONDS: f64 = 1.0 / 60.0;

pub struct FoxLua {
    pub lua: Lua,
    pub vfs: Arc<Vfs>,
    pub trace: Arc<Mutex<Trace>>,
    pub clock: Arc<Mutex<Clock>>,
    /// Things the engine-side systems did during frame ticks (block loads,
    /// UI callbacks), for the game's event log.
    pub events: Mutex<Vec<String>>,
}

impl FoxLua {
    pub fn new(vfs: Arc<Vfs>) -> mlua::Result<Self> {
        // The game uses the full standard library, including io/os/debug.
        let lua = unsafe { Lua::unsafe_new() };
        let trace = Arc::new(Mutex::new(Trace::default()));
        let host = Self {
            lua,
            vfs,
            trace,
            clock: Arc::default(),
            events: Mutex::default(),
        };

        {
            let t = host.trace.clone();
            let f = host
                .lua
                .create_function(move |_, (path, _nargs): (String, u32)| {
                    // Strip the phantom "()" suffixes so chained calls group by API.
                    let key = path.replace("()", "");
                    *t.lock().unwrap().phantom_calls.entry(key).or_default() += 1;
                    Ok(())
                })?;
            host.lua.globals().set("__fox_trace", f)?;
        }
        host.lua
            .load(include_str!("prelude.lua"))
            .set_name("@fox-lua/prelude.lua")
            .exec()?;
        natives::install(&host)?;
        Ok(host)
    }

    /// Load and run a script from the archives. If it returns a table, the
    /// table is published as a global named after the file (how
    /// `Script.LoadLibrary` exposes `/Assets/tpp/script/lib/TppMain.lua` as
    /// `TppMain`).
    pub fn run_file(&self, path: &str) -> mlua::Result<Value> {
        run_file(&self.lua, &self.vfs, &self.trace, path)
    }

    /// Advance one frame: clock, timers, and the engine systems that make
    /// progress between script updates (script block loads, native UI flows).
    /// This runs between coroutine resumes too, so scripts that wait inside
    /// `OnInitialize` see their loads complete.
    pub fn advance_frame(&self) -> mlua::Result<()> {
        let now = {
            let mut c = self.clock.lock().unwrap();
            c.frame += 1;
            c.seconds += FRAME_SECONDS;
            c.seconds
        };
        messages::tick_timers(&self.lua, now);
        for (block, packages) in script_block::tick(&self.lua, &self.vfs)? {
            self.events
                .lock()
                .unwrap()
                .push(format!("script block {block} loaded {packages}"));
        }
        if let Some(e) = ui::tick(&self.lua)? {
            self.events.lock().unwrap().push(e);
        }
        Ok(())
    }

    /// Take the events recorded by [`FoxLua::advance_frame`].
    pub fn take_events(&self) -> Vec<String> {
        std::mem::take(&mut self.events.lock().unwrap())
    }

    /// Run a script as a coroutine, resuming it once per simulated frame
    /// until it finishes. Boot scripts (`/Tpp/start.lua`) yield while they
    /// wait for asynchronous loads. Returns the number of frames taken.
    pub fn run_coroutine(&self, path: &str, max_frames: usize) -> mlua::Result<usize> {
        let src = self
            .vfs
            .read(path)
            .map_err(|e| mlua::Error::runtime(format!("{path}: {e}")))?;
        self.trace.lock().unwrap().loaded.push(path.to_string());
        let func = self
            .lua
            .load(&src[..])
            .set_name(format!("@{path}"))
            .into_function()?;
        let thread = self.lua.create_thread(func)?;
        let mut frames = 0;
        loop {
            thread.resume::<mlua::MultiValue>(())?;
            if thread.status() != mlua::ThreadStatus::Resumable {
                return Ok(frames);
            }
            frames += 1;
            self.advance_frame()?;
            if frames >= max_frames {
                return Err(mlua::Error::runtime(format!(
                    "{path} still running after {max_frames} frames"
                )));
            }
        }
    }
}

/// Engine paths are rooted at `/`; scripts sometimes omit the slash
/// (`Script.LoadLibraryAsync"Tpp/Scripts/Equip/EquipIdTable.lua"`).
pub(crate) fn engine_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

pub(crate) fn run_file(
    lua: &Lua,
    vfs: &Vfs,
    trace: &Mutex<Trace>,
    path: &str,
) -> mlua::Result<Value> {
    let path = engine_path(path);
    let src = match vfs.read(&path) {
        Ok(s) => s,
        Err(_) => {
            trace.lock().unwrap().missing.push(path);
            return Ok(Value::Nil);
        }
    };
    trace.lock().unwrap().loaded.push(path.clone());
    let value: Value = lua.load(&src[..]).set_name(format!("@{path}")).eval()?;
    if let Value::Table(module) = &value {
        let stem = path.rsplit('/').next().unwrap_or(&path);
        let stem = stem.split('.').next().unwrap_or(stem);
        lua.globals().raw_set(stem, module.clone())?;
        load_requires(lua, vfs, trace, module)?;
    }
    Ok(value)
}

/// A library whose table has a `requires` array pulls those libraries in as
/// dependencies, each loaded once (the engine's script resource loader does
/// this at 0x140104d80). `Tpp.lua` uses it to load the 52 core libraries,
/// including `TppMain` and `TppSequence`.
fn load_requires(
    lua: &Lua,
    vfs: &Vfs,
    trace: &Mutex<Trace>,
    module: &mlua::Table,
) -> mlua::Result<()> {
    let Ok(mlua::Value::Table(reqs)) = module.raw_get::<Value>("requires") else {
        return Ok(());
    };
    for req in reqs.sequence_values::<String>() {
        let req = engine_path(&req?);
        let already = trace.lock().unwrap().loaded.contains(&req);
        if !already {
            run_file(lua, vfs, trace, &req)?;
        }
    }
    Ok(())
}

/// Coerce a script value to a number the way the engine would, except that
/// phantoms (native constants we have not recovered yet) read as 0 and are
/// recorded in the trace.
pub(crate) fn lua_number(trace: &Mutex<Trace>, lua: &Lua, v: &Value) -> mlua::Result<f64> {
    match v {
        Value::Integer(i) => Ok(*i as f64),
        Value::Number(n) => Ok(*n),
        Value::String(s) => s
            .to_str()?
            .trim()
            .parse()
            .map_err(|_| mlua::Error::runtime("string is not a number")),
        Value::Table(_) => {
            let name: Value = lua.globals().get::<Function>("tostring")?.call(v.clone())?;
            let name = name.to_string().unwrap_or_default();
            if let Some(path) = name
                .strip_prefix("<phantom ")
                .and_then(|s| s.strip_suffix('>'))
            {
                *trace
                    .lock()
                    .unwrap()
                    .phantom_values
                    .entry(path.to_string())
                    .or_default() += 1;
                Ok(0.0)
            } else {
                Err(mlua::Error::runtime("table is not a number"))
            }
        }
        _ => Err(mlua::Error::runtime(format!(
            "expected number, got {}",
            v.type_name()
        ))),
    }
}
