//! Mission loading, as the native mission block does it.
//!
//! A mission package (`.fpkd`) contains a `TppSimpleMissionData` entity whose
//! `script` is the mission shim (`/Assets/tpp/script/mission/mission_main.lua`)
//! and whose `subScripts` map names (`sequence`, `enemy`, `demo`, `radio`,
//! `sound`) to scripts in the same package. The shim forwards its lifecycle
//! hooks to `TppMain`. The engine calls each hook with a table holding only the
//! sub-scripts (`{sequence = ..., enemy = ...}`): `TppMain.OnAllocate` walks it
//! with `pairs` and expects every value to be a script table.

use fox_archive::Fpk;
use fox_data::{DataSet, Value as FoxValue};
use mlua::{Function, Table, Value};

use crate::FoxLua;

/// Classes that carry mission script wiring.
const MISSION_DATA_CLASSES: &[&str] = &["TppSimpleMissionData", "TppMissionData"];

pub struct Mission {
    pub package: String,
    /// The shim's table: lifecycle hooks.
    pub main: Table,
    /// Argument passed to every hook: sub-script tables by key.
    pub scripts: Table,
    /// Sub-scripts that were attached, by key.
    pub sub_scripts: Vec<(String, String)>,
}

fn lua_err(e: impl std::fmt::Display) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}

impl FoxLua {
    /// Load a mission package and build its mission table.
    pub fn load_mission(&self, package: &str) -> mlua::Result<Mission> {
        let data = self
            .vfs
            .read(package)
            .map_err(|e| lua_err(format!("{package}: {e}")))?;
        let pkg = Fpk::parse(&data).map_err(lua_err)?;
        let file = |path: &str| -> mlua::Result<Vec<u8>> {
            let e = pkg
                .entries
                .iter()
                .find(|e| e.clean_path() == path)
                .ok_or_else(|| lua_err(format!("{path} not in {package}")))?;
            pkg.read(e).map_err(lua_err)
        };

        // Find the mission data entity in the package's DataSets.
        let empty = fox_hash::str_code64(b"");
        let classes: Vec<u64> = MISSION_DATA_CLASSES
            .iter()
            .map(|c| fox_hash::str_code64(c.as_bytes()))
            .collect();
        let mut wiring = None;
        for e in pkg
            .entries
            .iter()
            .filter(|e| e.clean_path().ends_with(".fox2"))
        {
            let ds = DataSet::parse(&pkg.read(e).map_err(lua_err)?).map_err(lua_err)?;
            if let Some(ent) = ds.entities.iter().find(|x| classes.contains(&x.class)) {
                let lit = |h: u64| ds.strings.get(&h).cloned().filter(|_| h != empty);
                let script = match ent.property("script").and_then(|p| p.values.first()) {
                    Some((_, FoxValue::String(h) | FoxValue::Path(h) | FoxValue::FilePtr(h))) => {
                        lit(*h)
                    }
                    _ => None,
                };
                let subs: Vec<(String, String)> = ent
                    .property("subScripts")
                    .map(|p| {
                        p.values
                            .iter()
                            .filter_map(|(k, v)| match (k, v) {
                                (
                                    Some(k),
                                    FoxValue::String(h) | FoxValue::Path(h) | FoxValue::FilePtr(h),
                                ) => Some((
                                    lit(*k).or_else(|| ds.strings.get(k).cloned())?,
                                    lit(*h)?,
                                )),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                wiring = Some((script, subs));
                break;
            }
        }
        let (script, subs) =
            wiring.ok_or_else(|| lua_err(format!("no mission data entity in {package}")))?;
        let script = script.ok_or_else(|| lua_err("mission data has no script"))?;

        let run = |path: &str| -> mlua::Result<Value> {
            let src = file(path)?;
            self.trace
                .lock()
                .unwrap()
                .loaded
                .push(format!("{package}:{path}"));
            let v: Value = self
                .lua
                .load(&src[..])
                .set_name(format!("@{path}"))
                .eval()?;
            // Published under the file stem, like Script.LoadLibrary
            // (mission scripts refer to each other as `s10010_sequence` etc).
            if let Value::Table(_) = &v {
                let stem = path
                    .rsplit('/')
                    .next()
                    .unwrap_or(path)
                    .split('.')
                    .next()
                    .unwrap_or(path);
                self.lua.globals().raw_set(stem, v.clone())?;
            }
            Ok(v)
        };

        let Value::Table(main) = run(&script)? else {
            return Err(lua_err(format!("{script} did not return a table")));
        };
        let scripts = self.lua.create_table()?;
        for (key, path) in &subs {
            scripts.set(key.as_str(), run(path)?)?;
        }
        Ok(Mission {
            package: package.to_string(),
            main,
            scripts,
            sub_scripts: subs,
        })
    }
}

impl Mission {
    /// Run a hook as a coroutine resumed once per frame until it returns
    /// (`TppMain.OnAllocate` yields while it waits for saves and loads).
    /// Returns the frames taken.
    pub fn run_hook(&self, host: &FoxLua, hook: &str, max_frames: usize) -> mlua::Result<usize> {
        let Value::Function(f) = self.main.get::<Value>(hook)? else {
            return Ok(0);
        };
        let thread = host.lua.create_thread(f)?;
        let mut frames = 0;
        let mut first = true;
        loop {
            if first {
                thread.resume::<mlua::MultiValue>(self.scripts.clone())?;
                first = false;
            } else {
                thread.resume::<mlua::MultiValue>(())?;
            }
            if thread.status() != mlua::ThreadStatus::Resumable {
                return Ok(frames);
            }
            frames += 1;
            host.advance_frame();
            if frames >= max_frames {
                return Err(lua_err(format!(
                    "{hook} still running after {max_frames} frames"
                )));
            }
        }
    }

    /// Call a lifecycle hook (`OnAllocate`, `OnInitialize`, `OnUpdate`, ...)
    /// with the sub-script table, if the mission defines it.
    pub fn call(&self, hook: &str) -> mlua::Result<()> {
        match self.main.get::<Value>(hook)? {
            Value::Function(f) => {
                let f: Function = f;
                f.call::<()>(self.scripts.clone())
            }
            _ => Ok(()),
        }
    }
}
