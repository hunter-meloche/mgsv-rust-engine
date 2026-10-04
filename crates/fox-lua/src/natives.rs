//! Rust implementations of Fox Engine native Lua modules.
//!
//! Each module is a real Lua table. Members we have not implemented fall
//! through to a traced phantom, so a module can be filled in one function at
//! a time.

use mlua::{Function, IntoLuaMulti, Lua, MultiValue, Table, Value};

use crate::{FoxLua, run_file};

/// Create (or fetch) a native module table whose unknown members are phantoms.
fn module(lua: &Lua, name: &str) -> mlua::Result<Table> {
    let g = lua.globals();
    if let Ok(Value::Table(t)) = g.raw_get::<Value>(name) {
        if t.metatable().is_some() {
            return Ok(t);
        }
    }
    let t = lua.create_table()?;
    let mt = lua.create_table()?;
    let phantom: Function = g.get("__fox_phantom")?;
    let prefix = name.to_string();
    mt.set(
        "__index",
        lua.create_function(move |_, (_t, key): (Table, String)| {
            phantom.call::<Value>(format!("{prefix}.{key}"))
        })?,
    )?;
    t.set_metatable(Some(mt))?;
    g.raw_set(name, t.clone())?;
    Ok(t)
}

/// Register `module.name` as a Rust function, counting calls in the trace.
fn def<A, R, F>(
    host: &FoxLua,
    module_table: &Table,
    module_name: &str,
    name: &str,
    f: F,
) -> mlua::Result<()>
where
    A: mlua::FromLuaMulti,
    R: IntoLuaMulti,
    F: Fn(&Lua, A) -> mlua::Result<R> + Send + 'static,
{
    let trace = host.trace.clone();
    let key = format!("{module_name}.{name}");
    let func = host.lua.create_function(move |lua, args: A| {
        *trace
            .lock()
            .unwrap()
            .native_calls
            .entry(key.clone())
            .or_default() += 1;
        f(lua, args)
    })?;
    module_table.raw_set(name, func)
}

fn log_line(
    host: &FoxLua,
    level: &'static str,
) -> impl Fn(&Lua, MultiValue) -> mlua::Result<()> + Send + 'static {
    let trace = host.trace.clone();
    move |_, args: MultiValue| {
        let parts: Vec<String> = args
            .iter()
            .map(|v| v.to_string().unwrap_or_else(|_| format!("{v:?}")))
            .collect();
        trace
            .lock()
            .unwrap()
            .log
            .push(format!("[{level}] {}", parts.join("\t")));
        Ok(())
    }
}

/// LuaBitOp 1.0 number normalisation: round to nearest (ties to even) and
/// wrap to a signed 32-bit integer.
fn tobit(x: f64) -> i32 {
    x.round_ties_even().rem_euclid(4_294_967_296.0) as u64 as u32 as i32
}

/// The `bit` library (LuaBitOp), which the engine links next to Lua 5.1.
fn install_bitop(host: &FoxLua) -> mlua::Result<()> {
    let lua = &host.lua;
    let bit = lua.create_table()?;
    let unary = |op: fn(i32) -> i32| {
        let t = host.trace.clone();
        move |lua: &Lua, x: Value| Ok(op(tobit(crate::lua_number(&t, lua, &x)?)))
    };
    let binary = |op: fn(i32, i32) -> i32| {
        let t = host.trace.clone();
        move |lua: &Lua, (x, n): (Value, Value)| {
            Ok(op(
                tobit(crate::lua_number(&t, lua, &x)?),
                tobit(crate::lua_number(&t, lua, &n)?),
            ))
        }
    };
    let fold = |op: fn(i32, i32) -> i32| {
        let t = host.trace.clone();
        move |lua: &Lua, args: mlua::Variadic<Value>| {
            let mut it = args.iter();
            let first = it
                .next()
                .ok_or_else(|| mlua::Error::runtime("bad argument #1 (number expected)"))?;
            let mut acc = tobit(crate::lua_number(&t, lua, first)?);
            for v in it {
                acc = op(acc, tobit(crate::lua_number(&t, lua, v)?));
            }
            Ok(acc)
        }
    };
    bit.set("tobit", lua.create_function(unary(|x| x))?)?;
    bit.set("bnot", lua.create_function(unary(|x| !x))?)?;
    bit.set("bswap", lua.create_function(unary(i32::swap_bytes))?)?;
    bit.set("band", lua.create_function(fold(|a, b| a & b))?)?;
    bit.set("bor", lua.create_function(fold(|a, b| a | b))?)?;
    bit.set("bxor", lua.create_function(fold(|a, b| a ^ b))?)?;
    bit.set(
        "lshift",
        lua.create_function(binary(|x, n| ((x as u32) << (n & 31)) as i32))?,
    )?;
    bit.set(
        "rshift",
        lua.create_function(binary(|x, n| ((x as u32) >> (n & 31)) as i32))?,
    )?;
    bit.set(
        "arshift",
        lua.create_function(binary(|x, n| x >> (n & 31)))?,
    )?;
    bit.set(
        "rol",
        lua.create_function(binary(|x, n| {
            (x as u32).rotate_left((n & 31) as u32) as i32
        }))?,
    )?;
    bit.set(
        "ror",
        lua.create_function(binary(|x, n| {
            (x as u32).rotate_right((n & 31) as u32) as i32
        }))?,
    )?;
    {
        let t = host.trace.clone();
        bit.set(
            "tohex",
            lua.create_function(move |lua, (x, n): (Value, Option<f64>)| {
                let n = n.map(tobit).unwrap_or(8);
                let (digits, upper) = (n.unsigned_abs().min(8) as usize, n < 0);
                let v = tobit(crate::lua_number(&t, lua, &x)?) as u32;
                let s = if upper {
                    format!("{v:08X}")
                } else {
                    format!("{v:08x}")
                };
                Ok(s[8 - digits..].to_string())
            })?,
        )?;
    }
    lua.globals().raw_set("bit", bit)
}

/// `SplashScreen`: timed full-screen logos shown during boot.
///
/// `Create(name, texture, ...)` returns a handle; `Show(handle, fade_in, hold,
/// fade_out)` starts it; `GetSplashScreenWithName(name)` returns the handle
/// until the splash has finished, then nil (start.lua polls this each frame).
fn install_splash_screen(host: &FoxLua) -> mlua::Result<()> {
    use std::collections::HashMap;

    struct Splash {
        handle: mlua::RegistryKey,
        ends_at: Option<f64>,
    }
    let splashes: std::sync::Arc<std::sync::Mutex<HashMap<String, Splash>>> = Default::default();
    let m = module(&host.lua, "SplashScreen")?;

    {
        let s = splashes.clone();
        def(
            host,
            &m,
            "SplashScreen",
            "Create",
            move |lua, (name, texture, _rest): (String, Option<String>, MultiValue)| {
                let handle = lua.create_table()?;
                handle.set("name", name.as_str())?;
                handle.set("texture", texture)?;
                s.lock().unwrap().insert(
                    name,
                    Splash {
                        handle: lua.create_registry_value(handle.clone())?,
                        ends_at: None,
                    },
                );
                Ok(handle)
            },
        )?;
    }
    {
        let (s, clock) = (splashes.clone(), host.clock.clone());
        def(
            host,
            &m,
            "SplashScreen",
            "Show",
            move |_,
                  (handle, fade_in, hold, fade_out): (
                Table,
                Option<f64>,
                Option<f64>,
                Option<f64>,
            )| {
                let name: String = handle.get("name")?;
                let now = clock.lock().unwrap().seconds;
                let total = fade_in.unwrap_or(0.0) + hold.unwrap_or(0.0) + fade_out.unwrap_or(0.0);
                if let Some(sp) = s.lock().unwrap().get_mut(&name) {
                    sp.ends_at = Some(now + total);
                }
                Ok(())
            },
        )?;
    }
    {
        let (s, clock) = (splashes.clone(), host.clock.clone());
        def(
            host,
            &m,
            "SplashScreen",
            "GetSplashScreenWithName",
            move |lua, name: String| {
                let now = clock.lock().unwrap().seconds;
                let mut map = s.lock().unwrap();
                let finished = match map.get(&name) {
                    None => return Ok(Value::Nil),
                    Some(sp) => sp.ends_at.is_some_and(|t| now >= t),
                };
                if finished {
                    if let Some(sp) = map.remove(&name) {
                        lua.remove_registry_value(sp.handle)?;
                    }
                    return Ok(Value::Nil);
                }
                lua.registry_value(&map[&name].handle)
            },
        )?;
    }
    Ok(())
}

pub fn install(host: &FoxLua) -> mlua::Result<()> {
    let lua = &host.lua;
    let g = lua.globals();

    // --- globals -------------------------------------------------------------
    g.raw_set(
        "StrCode32",
        lua.create_function(|_, s: mlua::String| Ok(fox_hash::str_code32(&s.as_bytes())))?,
    )?;
    g.raw_set("print", lua.create_function(log_line(host, "print"))?)?;
    {
        let (vfs, trace) = (host.vfs.clone(), host.trace.clone());
        // dofile resolves through the archives; engine paths are rooted at "/".
        g.raw_set(
            "dofile",
            lua.create_function(move |lua, path: String| {
                let p = if path.starts_with('/') {
                    path
                } else {
                    format!("/{path}")
                };
                run_file(lua, &vfs, &trace, &p)
            })?,
        )?;
    }

    {
        let (vfs, trace) = (host.vfs.clone(), host.trace.clone());
        // loadfile likewise; returns (nil, message) when the file is absent.
        g.raw_set(
            "loadfile",
            lua.create_function(move |lua, path: String| {
                let p = if path.starts_with('/') {
                    path
                } else {
                    format!("/{path}")
                };
                match vfs.read(&p) {
                    Ok(src) => {
                        trace.lock().unwrap().loaded.push(p.clone());
                        let f = lua
                            .load(&src[..])
                            .set_name(format!("@{p}"))
                            .into_function()?;
                        Ok((Value::Function(f), Value::Nil))
                    }
                    Err(e) => {
                        trace.lock().unwrap().missing.push(p.clone());
                        Ok((
                            Value::Nil,
                            Value::String(lua.create_string(format!("cannot open {p}: {e}"))?),
                        ))
                    }
                }
            })?,
        )?;
    }

    // --- Script ----------------------------------------------------------------
    let script = module(lua, "Script")?;
    for name in ["LoadLibrary", "LoadLibraryAsync"] {
        let (vfs, trace) = (host.vfs.clone(), host.trace.clone());
        def(host, &script, "Script", name, move |lua, path: String| {
            run_file(lua, &vfs, &trace, &path)
        })?;
    }
    // Loading is synchronous here, so nothing is ever pending.
    def(
        host,
        &script,
        "Script",
        "IsLoadingLibrary",
        |_, _: MultiValue| Ok(false),
    )?;

    // --- Fox -------------------------------------------------------------------
    let fox = module(lua, "Fox")?;
    def(host, &fox, "Fox", "GetPlatformName", |_, ()| Ok("Windows"))?;
    def(host, &fox, "Fox", "StrCode32", |_, s: mlua::String| {
        Ok(fox_hash::str_code32(&s.as_bytes()))
    })?;
    def(host, &fox, "Fox", "StrCode64", |_, s: mlua::String| {
        Ok(fox_hash::str_code64(&s.as_bytes()) as f64)
    })?;
    def(
        host,
        &fox,
        "Fox",
        "PathFileNameCode32",
        |_, s: mlua::String| Ok(fox_hash::path_code64_ext(&s.to_str()?) as u32),
    )?;
    for (name, level) in [
        ("Log", "log"),
        ("Warning", "warning"),
        ("Error", "error"),
        ("Print", "print"),
    ] {
        let f = log_line(host, level);
        def(host, &fox, "Fox", name, move |lua, a: MultiValue| f(lua, a))?;
    }
    def(host, &fox, "Fox", "SetBreakIgnore", |_, _: MultiValue| {
        Ok(())
    })?;

    crate::constants::install(lua, |lua, name| module(lua, name))?;
    install_bitop(host)?;
    crate::script_vars::install(lua, host.trace.clone())?;
    install_splash_screen(host)?;
    crate::messages::install(lua, host.clock.clone(), |l, n| module(l, n))?;
    crate::mission_manager::install(lua, |l, n| module(l, n))?;
    crate::ui::install(lua, |l, n| module(l, n))?;

    // --- ScriptBlock ------------------------------------------------------------
    crate::script_block::install(lua, |l, n| module(l, n))?;

    // --- StageBlock -------------------------------------------------------------
    {
        // No stage (terrain block grid) is streamed yet, so the current
        // minimum small-block index is the grid origin. Returns (x, y).
        // TODO: derive from the player position once stages load.
        let sb = module(lua, "StageBlock")?;
        def(
            host,
            &sb,
            "StageBlock",
            "GetCurrentMinimumSmallBlockIndex",
            |_, _: MultiValue| Ok((0, 0)),
        )?;
    }

    // --- TppEquip ----------------------------------------------------------------
    {
        let equip = module(lua, "TppEquip")?;
        let vfs = host.vfs.clone();
        // Existence is a lookup in the archive index (start.lua uses it to
        // pick between parameter files).
        def(
            host,
            &equip,
            "TppEquip",
            "IsExistFile",
            move |_, path: String| Ok(vfs.lookup(&crate::engine_path(&path)).is_some()),
        )?;
    }

    // --- fox (input constants; values still to be recovered from the binary) --
    module(lua, "fox")?;

    // --- foxmath ---------------------------------------------------------------
    let fm = module(lua, "foxmath")?;
    fm.raw_set("PI", std::f64::consts::PI)?;
    def(host, &fm, "foxmath", "DegreeToRadian", |_, d: f64| {
        Ok(d.to_radians())
    })?;
    def(host, &fm, "foxmath", "RadianToDegree", |_, r: f64| {
        Ok(r.to_degrees())
    })?;
    def(host, &fm, "foxmath", "Atan2", |_, (y, x): (f64, f64)| {
        Ok(y.atan2(x))
    })?;
    def(host, &fm, "foxmath", "Absf", |_, v: f64| Ok(v.abs()))?;
    def(host, &fm, "foxmath", "Round", |_, v: f64| Ok(v.round()))?;
    def(host, &fm, "foxmath", "NormalizeRadian", |_, r: f64| {
        use std::f64::consts::{PI, TAU};
        Ok(r - TAU * ((r + PI) / TAU).floor())
    })?;
    {
        let state = std::sync::atomic::AtomicU64::new(0x9E37_79B9_7F4A_7C15);
        def(host, &fm, "foxmath", "FRnd", move |_, ()| {
            // xorshift64*; the engine's own generator is not reproduced yet.
            let mut x = state.load(std::sync::atomic::Ordering::Relaxed);
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            state.store(x, std::sync::atomic::Ordering::Relaxed);
            Ok((x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64)
        })?;
    }

    Ok(())
}
