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

fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split('.').next().unwrap_or(name)
}

/// Order sub-scripts so that a script runs after the sub-scripts whose
/// globals it references (`s10010_sequence` uses `s10010_demo` while it
/// loads). Scripts without such references keep their DataSet order; cycles
/// fall back to that order too.
pub fn sub_script_load_order(
    subs: &[(String, String)],
    source: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<usize> {
    let stems: Vec<&str> = subs.iter().map(|(_, p)| file_stem(p)).collect();
    let deps: Vec<Vec<usize>> = subs
        .iter()
        .enumerate()
        .map(|(i, (_, p))| {
            let src = source(p).unwrap_or_default();
            let src = String::from_utf8_lossy(&src);
            (0..subs.len())
                .filter(|&j| j != i && src.contains(&format!("{}.", stems[j])))
                .collect()
        })
        .collect();
    let mut order = Vec::with_capacity(subs.len());
    let mut done = vec![false; subs.len()];
    while order.len() < subs.len() {
        // First not-yet-loaded script whose dependencies are all loaded;
        // if none (a cycle), the first remaining one.
        let next = (0..subs.len())
            .find(|&i| !done[i] && deps[i].iter().all(|&d| done[d]))
            .or_else(|| (0..subs.len()).find(|&i| !done[i]))
            .expect("some script remains");
        done[next] = true;
        order.push(next);
    }
    order
}

/// `ScriptBlockData` entities in a DataSet (enabled ones only).
pub fn script_blocks_in(ds: &DataSet) -> Vec<crate::script_block::Block> {
    let class = fox_hash::str_code64(b"ScriptBlockData");
    let lit = |h: u64| ds.strings.get(&h).cloned().unwrap_or_default();
    ds.entities
        .iter()
        .filter(|e| e.class == class)
        .filter(|e| {
            !matches!(
                e.property("enabled").and_then(|p| p.values.first()),
                Some((_, FoxValue::Bool(false)))
            )
        })
        .filter_map(|e| {
            let name = match e.property("name").and_then(|p| p.values.first()) {
                Some((_, FoxValue::String(h))) => lit(*h),
                _ => return None,
            };
            let size = match e.property("sizeInBytes").and_then(|p| p.values.first()) {
                Some((_, FoxValue::U32(n))) => *n,
                _ => 0,
            };
            let prerequisites = e
                .property("prerequisites")
                .map(|p| {
                    p.values
                        .iter()
                        .filter_map(|(_, v)| match v {
                            FoxValue::String(h) => Some(lit(*h)),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(crate::script_block::Block {
                name,
                size,
                prerequisites,
                state: crate::script_block::STATE_EMPTY,
                packages: Vec::new(),
                activate_on_load: false,
            })
        })
        .collect()
}

/// The data half of a package: `x.fpk` → `x.fpkd` (scripts and DataSets live
/// in the `.fpkd`).
pub fn data_package_path(path: &str) -> String {
    if path.ends_with(".fpk") {
        format!("{path}d")
    } else {
        path.to_string()
    }
}

impl FoxLua {
    /// Load a mission from one package.
    pub fn load_mission(&self, package: &str) -> mlua::Result<Mission> {
        self.load_mission_packages(&[package.to_string()])
    }

    /// Load a mission from the packages the scripts' mission package path
    /// function returned (`.fpk` paths are mapped to their `.fpkd`). The
    /// mission data entity and its scripts may be in any of them.
    pub fn load_mission_packages(&self, packages: &[String]) -> mlua::Result<Mission> {
        let mut blobs = Vec::new();
        for p in packages {
            let d = data_package_path(p);
            if let Ok(bytes) = self.vfs.read(&d) {
                blobs.push((d, bytes));
            }
        }
        let pkgs: Vec<(&str, Fpk)> = blobs
            .iter()
            .filter_map(|(p, b)| Fpk::parse(b).ok().map(|f| (p.as_str(), f)))
            .collect();
        let package = pkgs.iter().map(|(p, _)| *p).collect::<Vec<_>>().join(" + ");
        let file = |path: &str| -> mlua::Result<Vec<u8>> {
            for (_, pkg) in &pkgs {
                if let Some(e) = pkg.entries.iter().find(|e| e.clean_path() == path) {
                    return pkg.read(e).map_err(lua_err);
                }
            }
            Err(lua_err(format!("{path} not in {package}")))
        };

        // Find the mission data entity in the packages' DataSets.
        let empty = fox_hash::str_code64(b"");
        let classes: Vec<u64> = MISSION_DATA_CLASSES
            .iter()
            .map(|c| fox_hash::str_code64(c.as_bytes()))
            .collect();
        let mut wiring = None;
        let mut declared_blocks = Vec::new();
        let block_class = fox_hash::str_code64(b"ScriptBlockData");
        let fox2s = pkgs.iter().flat_map(|(_, pkg)| {
            pkg.entries
                .iter()
                .filter(|e| e.clean_path().ends_with(".fox2"))
                .map(move |e| (pkg, e))
        });
        for (pkg, e) in fox2s {
            let ds = DataSet::parse(&pkg.read(e).map_err(lua_err)?).map_err(lua_err)?;
            declared_blocks.extend(script_blocks_in(&ds));
            let _ = block_class;
            if wiring.is_some() {
                continue;
            }
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
            }
        }
        let (script, subs) =
            wiring.ok_or_else(|| lua_err(format!("no mission data entity in {package}")))?;
        crate::script_block::declare(&self.lua, declared_blocks);
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
        // Loose scripts in the packages (`title_sequence.lua`,
        // `cypr_mission_block.lua`, ...) are not named by any entity; the
        // engine runs every package script as a library. Run them before the
        // mission scripts, which use them.
        let wired: std::collections::HashSet<&str> = std::iter::once(script.as_str())
            .chain(subs.iter().map(|(_, p)| p.as_str()))
            .collect();
        let loose: Vec<(String, String)> = pkgs
            .iter()
            .flat_map(|(_, pkg)| pkg.entries.iter().map(|e| e.clean_path()))
            .filter(|p| p.ends_with(".lua") && !wired.contains(p.as_str()))
            .map(|p| (String::new(), p))
            .collect();
        for i in sub_script_load_order(&loose, |p| file(p).ok()) {
            run(&loose[i].1)?;
        }

        let scripts = self.lua.create_table()?;
        for i in sub_script_load_order(&subs, |p| file(p).ok()) {
            let (key, path) = &subs[i];
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
    /// Deliver pending `svars` change notifications to `OnChangeSVars(name,
    /// index)` (index is 0-based for arrays, nil for scalars). Handlers may
    /// write further svars, so this repeats until the queue is empty.
    pub fn deliver_svars_changes(&self, host: &FoxLua) -> mlua::Result<usize> {
        let Value::Function(hook) = self.main.get::<Value>("OnChangeSVars")? else {
            crate::script_vars::take_svars_changes(&host.lua);
            return Ok(0);
        };
        let mut delivered = 0;
        for _ in 0..64 {
            let changes = crate::script_vars::take_svars_changes(&host.lua);
            if changes.is_empty() {
                break;
            }
            for (name, index) in changes {
                hook.call::<()>((self.scripts.clone(), name, index))?;
                delivered += 1;
            }
        }
        Ok(delivered)
    }

    /// Deliver queued engine messages to `OnMessage(class, message, arg0..3)`.
    /// Messages whose handler returns `Mission.ON_MESSAGE_RESULT_RESEND` are
    /// queued again for the next frame.
    pub fn deliver_messages(&self, host: &FoxLua) -> mlua::Result<usize> {
        use crate::messages;
        let msgs = messages::take(&host.lua);
        let Value::Function(hook) = self.main.get::<Value>("OnMessage")? else {
            return Ok(0);
        };
        let n = msgs.len();
        for m in msgs {
            messages::set_current_resend(&host.lua, m.resend);
            let [a0, a1, a2, a3] = m.args.clone();
            let r: Value = hook.call((self.scripts.clone(), m.class, m.id, a0, a1, a2, a3))?;
            let resend = matches!(r, Value::Integer(i) if i == messages::RESEND)
                || matches!(r, Value::Number(f) if f == messages::RESEND as f64);
            if resend && m.resend < 600 {
                messages::requeue(&host.lua, m);
            }
            self.deliver_svars_changes(host)?;
        }
        messages::set_current_resend(&host.lua, 0);
        Ok(n)
    }

    /// One game frame: `OnUpdate`, svars notifications, then messages.
    pub fn update(&self, host: &FoxLua) -> mlua::Result<()> {
        self.call("OnUpdate")?;
        self.deliver_svars_changes(host)?;
        self.deliver_messages(host)?;
        Ok(())
    }

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
            self.deliver_svars_changes(host)?;
            self.deliver_messages(host)?;
            if thread.status() != mlua::ThreadStatus::Resumable {
                return Ok(frames);
            }
            frames += 1;
            host.advance_frame()?;
            if frames >= max_frames {
                // Where the coroutine is suspended tells us what it waits for.
                let traceback: String = host
                    .lua
                    .globals()
                    .get::<Table>("debug")?
                    .get::<Function>("traceback")?
                    .call((thread.clone(), "suspended at:"))
                    .unwrap_or_default();
                return Err(lua_err(format!(
                    "{hook} still running after {max_frames} frames\n{traceback}"
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
