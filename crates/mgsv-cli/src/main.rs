use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use fox_archive::{Fpk, Vfs, find_game_dir, fpk};
use rayon::prelude::*;

/// Tools for a local MGSV: TPP install. Nothing here leaves your machine.
#[derive(Parser)]
#[command(name = "mgsv")]
struct Cli {
    /// Game directory. Defaults to MGSV_GAME_DIR or the nearest ancestor
    /// containing mgsvtpp.exe.
    #[arg(long, global = true)]
    game: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Summarise mounted archives and name-resolution coverage.
    Info,
    /// List files. FILTER is a substring match on the resolved path.
    Ls {
        filter: Option<String>,
        /// Only list entries whose name is resolved.
        #[arg(long)]
        named: bool,
    },
    /// Print one file to stdout.
    Cat { path: String },
    /// Extract files into OUT (default: <repo>/extracted, which is gitignored).
    Extract {
        filter: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also unpack .fpk/.fpkd packages into <name>_fpk(d)/ folders.
        #[arg(long)]
        unpack_fpk: bool,
    },
    /// List the contents of an .fpk/.fpkd inside the archives.
    Fpk { path: String },
    /// Run the game's boot scripts (init.lua, start.lua, start2nd.lua) in the
    /// Rust Lua host and report which native APIs they touched.
    Boot {
        /// Print every unimplemented native that was called.
        #[arg(long)]
        all: bool,
    },
    /// Boot, then run the game loop from the init mission (or --mission-code),
    /// following the scripts' mission switches. Prints events and sequence
    /// changes.
    Run {
        /// Frames to run after boot.
        #[arg(long, default_value_t = 600)]
        frames: usize,
        /// First mission code (default: init, 1).
        #[arg(long, default_value_t = fox_lua::game::INIT_MISSION_CODE)]
        mission_code: i64,
        /// Lua expression to print whenever its value changes (repeatable),
        /// e.g. --watch mvars.seq_missionPrepareState
        #[arg(long)]
        watch: Vec<String>,
        /// Print every unimplemented native that was called.
        #[arg(long)]
        all: bool,
        /// Post an engine message at a frame, standing in for player input:
        /// FRAME:CLASS:MESSAGE[:SENDER] (repeatable), e.g. 200:UI:PressStart
        #[arg(long)]
        post: Vec<fox_lua::game::ScheduledMessage>,
        /// Log every engine message posted (fades, timers, UI).
        #[arg(long)]
        messages: bool,
    },
    /// Dump a .fox2 DataSet. PATH is an archive path, or `PACKAGE:INNER` for
    /// a file inside an .fpk/.fpkd (e.g. `/Assets/.../init.fpkd:/Assets/.../init.fox2`).
    Fox2 { path: String },
    /// Parse every .fox2 inside every .fpkd and report failures.
    Fox2Check,
    /// Summarise an .fmdl model (PACKAGE.fpk:INNER for files inside packages).
    Fmdl { path: String },
    /// Parse and validate every .fmdl inside every .fpk.
    FmdlCheck,
    /// Find files inside packages whose path contains PATTERN; prints
    /// `PACKAGE:INNER` lines usable with fox2/fmdl.
    Find { pattern: String },
    /// List the clips in an .mtar motion archive (PACKAGE.fpk:INNER).
    Mtar { path: String },
    /// Decode every clip in every .mtar inside every package.
    MtarCheck,
    /// Harvest bone names (exe identifiers, model string tables, script
    /// strings) into re/out/bone_names.txt (local) and report how many player
    /// animation / model bones they resolve.
    BoneNames,
}

/// Read a file from the archives, or from inside a package with `PKG:INNER`.
fn read_any(vfs: &Vfs, path: &str) -> Result<Vec<u8>> {
    if let Some((pkg, inner)) = path
        .split_once(".fpkd:")
        .map(|(a, b)| (format!("{a}.fpkd"), b))
        .or_else(|| {
            path.split_once(".fpk:")
                .map(|(a, b)| (format!("{a}.fpk"), b))
        })
    {
        let data = vfs.read(&pkg)?;
        let p = Fpk::parse(&data)?;
        let e = p
            .entries
            .iter()
            .find(|e| e.clean_path() == inner)
            .with_context(|| format!("{inner} not in {pkg}"))?;
        return Ok(p.read(e)?);
    }
    Ok(vfs.read(path)?)
}

/// Names for .fox2 hashes: FoxTool's dictionary plus identifiers harvested
/// from the user's own mgsvtpp.exe.
fn fox_names(vfs: &Vfs) -> fox_data::NameTable {
    let mut names = fox_data::NameTable::new();
    let _ = names.load_dictionary(repo_dir().join("dictionaries/fox_dictionary.txt"));
    let _ = names.load_dictionary(repo_dir().join("re/out/bone_names.txt"));
    if let Ok(exe) = std::fs::read(vfs.game_dir.join("mgsvtpp.exe")) {
        names.harvest_identifiers(&exe);
    }
    names
}

fn repo_dir() -> PathBuf {
    // crates/mgsv-cli -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_default()
}

fn open_vfs(game: Option<PathBuf>) -> Result<Vfs> {
    let dir = match game {
        Some(d) => d,
        None => find_game_dir()
            .or_else(|| repo_dir().parent().map(Path::to_path_buf))
            .context("could not find the game; pass --game or set MGSV_GAME_DIR")?,
    };
    let mut vfs = Vfs::open(&dir).with_context(|| format!("opening {}", dir.display()))?;
    let dicts = repo_dir().join("dictionaries");
    for name in ["qar_dictionary.txt", "harvested_paths.txt"] {
        let p = dicts.join(name);
        if p.is_file() {
            vfs.dictionary.load_file(&p)?;
        }
    }
    Ok(vfs)
}

fn main() -> Result<()> {
    match run() {
        // `mgsv ls | head` closes the pipe early; that is not an error.
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        r => r,
    }
}

fn run() -> Result<()> {
    use std::io::Write;
    let cli = Cli::parse();
    let vfs = open_vfs(cli.game)?;
    let mut out = std::io::stdout().lock();
    match cli.cmd {
        Cmd::Info => info(&vfs),
        Cmd::Ls { filter, named } => {
            let mut rows: Vec<_> = vfs
                .iter()
                .map(|(h, r)| (vfs.dictionary.resolve(h), r))
                .filter(|((n, ok), _)| {
                    (!named || *ok) && filter.as_ref().is_none_or(|f| n.contains(f.as_str()))
                })
                .collect();
            rows.sort_by(|a, b| a.0.0.cmp(&b.0.0));
            for ((name, _), r) in rows {
                let (a, e) = vfs.entry(r);
                let arch = a.path().file_name().unwrap().to_string_lossy();
                writeln!(out, "{:>10}  {:<12} {}", e.uncompressed_size, arch, name)?;
            }
            Ok(())
        }
        Cmd::Cat { path } => {
            out.write_all(&vfs.read(&path)?)?;
            Ok(())
        }
        Cmd::Extract {
            filter,
            out,
            unpack_fpk,
        } => extract(&vfs, filter, out, unpack_fpk),
        Cmd::Fpk { path } => {
            let data = vfs.read(&path)?;
            let pkg = Fpk::parse(&data)?;
            writeln!(
                out,
                "{:?}, {} entries, {} references",
                pkg.kind,
                pkg.entries.len(),
                pkg.references.len()
            )?;
            for e in &pkg.entries {
                let mark = if e.path_verified { ' ' } else { '?' };
                writeln!(out, "{:>10} {mark} {}", e.size, e.clean_path())?;
            }
            for r in &pkg.references {
                writeln!(out, "  ref {r}")?;
            }
            Ok(())
        }
        Cmd::Boot { all } => boot(vfs, all),
        Cmd::Run {
            frames,
            mission_code,
            watch,
            all,
            post,
            messages,
        } => run_game(vfs, frames, mission_code, &watch, all, post, messages),
        Cmd::Fox2 { path } => {
            let ds = fox_data::DataSet::parse(&read_any(&vfs, &path)?)?;
            out.write_all(ds.to_text(&fox_names(&vfs)).as_bytes())?;
            Ok(())
        }
        Cmd::Fox2Check => fox2_check(&vfs),
        Cmd::Fmdl { path } => {
            let m = fox_data::Fmdl::parse(&read_any(&vfs, &path)?)?;
            let names = fox_names(&vfs);
            let verts: usize = m.meshes.iter().map(|x| x.positions.len()).sum();
            let tris: usize = m.meshes.iter().map(|x| x.indices.len() / 3).sum();
            writeln!(
                out,
                "fmdl v{}: {} bones, {} meshes, {verts} vertices, {tris} triangles, {} materials, {} textures",
                m.version,
                m.bones.len(),
                m.meshes.len(),
                m.material_instances.len(),
                m.textures.len()
            )?;
            for (i, b) in m.bones.iter().enumerate().take(12) {
                writeln!(
                    out,
                    "  bone {i:>3} {} parent {}",
                    m.name(b.name, &names),
                    b.parent
                )?;
            }
            for (i, g) in m.mesh_groups.iter().enumerate() {
                writeln!(out, "  group {i:>3} {}", m.name(g.name, &names))?;
            }
            match m.validate() {
                Ok(()) => writeln!(out, "valid")?,
                Err(e) => writeln!(out, "INVALID: {e}")?,
            }
            Ok(())
        }
        Cmd::FmdlCheck => fmdl_check(&vfs),
        Cmd::Mtar { path } => {
            let data = read_any(&vfs, &path)?;
            let m = fox_data::mtar::Mtar::parse(&data)?;
            writeln!(
                out,
                "mtar v{}: {} clips, {} track units, {} segments, flags {:#x}, {} skeleton names",
                m.version,
                m.entries.len(),
                m.unit_count,
                m.segment_count,
                m.flags,
                m.skeleton.len()
            )?;
            let bone = |h: u32| -> String {
                m.skeleton
                    .iter()
                    .find(|n| fox_hash::str_code32(n.as_bytes()) == h)
                    .cloned()
                    .unwrap_or_else(|| format!("{h:#010x}"))
            };
            let units: Vec<String> = m.layout.iter().map(|u| bone(u.name)).collect();
            writeln!(out, "units: {}", units.join(" "))?;
            for i in 0..m.entries.len() {
                let (name, _) = vfs.dictionary.resolve(m.entries[i].hash);
                match m.clip(&data, i) {
                    Ok(c) => {
                        let keys: usize = c
                            .tracks
                            .iter()
                            .flat_map(|t| &t.channels)
                            .map(|ch| ch.keys.len())
                            .sum();
                        let animated = c
                            .tracks
                            .iter()
                            .filter(|t| t.flags & fox_data::mtar::UNIT_STATIC == 0)
                            .count();
                        writeln!(
                            out,
                            "  {:>4} frames, {:>3}/{} units animated, {:>6} keys  {name}",
                            c.frame_count,
                            animated,
                            c.tracks.len(),
                            keys
                        )?;
                    }
                    Err(e) => writeln!(out, "  FAILED {name}: {e}")?,
                }
            }
            Ok(())
        }
        Cmd::MtarCheck => mtar_check(&vfs),
        Cmd::BoneNames => bone_names(&vfs),
        Cmd::Find { pattern } => {
            let pkgs: Vec<_> = vfs
                .iter()
                .filter(|(h, _)| {
                    matches!(
                        fox_hash::extension_from_id((h >> 51) as u16),
                        Some("fpk" | "fpkd")
                    )
                })
                .collect();
            let mut hits: Vec<String> = pkgs
                .par_iter()
                .flat_map(|(h, r)| {
                    let pkg_name = vfs.dictionary.resolve(*h).0;
                    let Ok(data) = vfs.read_ref(*r) else {
                        return Vec::new();
                    };
                    let Ok(pkg) = Fpk::parse(&data) else {
                        return Vec::new();
                    };
                    pkg.entries
                        .iter()
                        .map(|e| e.clean_path())
                        .filter(|p| p.contains(pattern.as_str()))
                        .map(|p| format!("{pkg_name}:{p}"))
                        .collect()
                })
                .collect();
            hits.sort();
            for h in hits {
                writeln!(out, "{h}")?;
            }
            Ok(())
        }
    }
}

fn bone_names(vfs: &Vfs) -> Result<()> {
    // Candidate names: identifiers in the exe, string tables of models that
    // still carry one (older FMDL versions), and quoted strings in scripts.
    let mut names = std::collections::BTreeSet::new();
    let ident = |s: &str| {
        s.len() >= 3
            && s.len() <= 64
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !s.as_bytes()[0].is_ascii_digit()
    };
    if let Ok(exe) = std::fs::read(vfs.game_dir.join("mgsvtpp.exe")) {
        for w in exe.split(|b| !(b.is_ascii_alphanumeric() || *b == b'_')) {
            if let Ok(s) = std::str::from_utf8(w) {
                if ident(s) {
                    names.insert(s.to_string());
                }
            }
        }
    }
    let pkgs: Vec<_> = vfs
        .iter()
        .filter(|(h, _)| {
            matches!(
                fox_hash::extension_from_id((h >> 51) as u16),
                Some("fpk" | "fpkd")
            )
        })
        .collect();
    let found: Vec<String> = pkgs
        .par_iter()
        .flat_map(|(_, r)| {
            let mut out = Vec::new();
            let Ok(data) = vfs.read_ref(*r) else {
                return out;
            };
            let Ok(pkg) = Fpk::parse(&data) else {
                return out;
            };
            for e in &pkg.entries {
                let p = e.clean_path();
                let Ok(bytes) = pkg.read(e) else { continue };
                if p.ends_with(".fmdl") {
                    if let Ok(m) = fox_data::Fmdl::parse(&bytes) {
                        out.extend(m.strings);
                    }
                } else if p.ends_with(".lua") {
                    let text = String::from_utf8_lossy(&bytes);
                    out.extend(text.split('"').skip(1).step_by(2).map(str::to_string));
                }
            }
            out
        })
        .collect();
    names.extend(found.into_iter().filter(|s| ident(s)));

    let dest = repo_dir().join("re/out/bone_names.txt");
    std::fs::create_dir_all(dest.parent().unwrap())?;
    let skl: Vec<&String> = names.iter().filter(|n| n.starts_with("SKL_")).collect();
    std::fs::write(
        &dest,
        skl.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(
            "
",
        ) + "
",
    )?;
    println!(
        "{} candidate names ({} SKL_ bone names -> {})",
        names.len(),
        skl.len(),
        dest.display()
    );

    let by32: std::collections::HashMap<u32, &String> = names
        .iter()
        .map(|n| (fox_hash::str_code32(n.as_bytes()), n))
        .collect();
    let by64: std::collections::HashMap<u64, &String> = names
        .iter()
        .map(|n| (fox_hash::str_code64(n.as_bytes()), n))
        .collect();
    let data = read_any(
        vfs,
        "/Assets/tpp/pack/player/motion/player2_location_motion.fpk:/Assets/tpp/motion/mtar/player2/player2_cqc.mtar",
    )?;
    let m = fox_data::mtar::Mtar::parse(&data)?;
    let named: Vec<String> = m
        .layout
        .iter()
        .map(|u| {
            by32.get(&u.name)
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{:#010x}", u.name))
        })
        .collect();
    println!(
        "player2_cqc.mtar units named: {}/{}: {}",
        m.layout
            .iter()
            .filter(|u| by32.contains_key(&u.name))
            .count(),
        m.layout.len(),
        named.join(" ")
    );
    let fm = fox_data::Fmdl::parse(&read_any(
        vfs,
        "/Assets/tpp/pack/player/parts/plparts_normal.fpk:/Assets/tpp/chara/sna/Scenes/sna0_main0_def.fmdl",
    )?)?;
    let hit = fm
        .bones
        .iter()
        .filter(|b| {
            fm.str_code64s
                .get(b.name as usize)
                .is_some_and(|h| by64.contains_key(&(h & 0xFFFF_FFFF_FFFF)))
        })
        .count();
    println!("sna0_main0_def.fmdl bones named: {hit}/{}", fm.bones.len());
    Ok(())
}

fn mtar_check(vfs: &Vfs) -> Result<()> {
    let pkgs: Vec<_> = vfs
        .iter()
        .filter(|(h, _)| fox_hash::extension_from_id((h >> 51) as u16) == Some("fpk"))
        .collect();
    let archives = AtomicUsize::new(0);
    let clips = AtomicUsize::new(0);
    let keys = AtomicUsize::new(0);
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    let failures = std::sync::Mutex::new(Vec::new());
    pkgs.par_iter().for_each(|(_, r)| {
        let Ok(data) = vfs.read_ref(*r) else { return };
        let Ok(pkg) = Fpk::parse(&data) else { return };
        for e in pkg
            .entries
            .iter()
            .filter(|e| e.clean_path().ends_with(".mtar"))
        {
            // The same archive ships in many packages; check each once.
            if !seen.lock().unwrap().insert(e.md5) {
                continue;
            }
            archives.fetch_add(1, Ordering::Relaxed);
            let Ok(bytes) = pkg.read(e) else { continue };
            match fox_data::mtar::Mtar::parse(&bytes) {
                Ok(m) => {
                    for i in 0..m.entries.len() {
                        match m.clip(&bytes, i) {
                            Ok(c) => {
                                clips.fetch_add(1, Ordering::Relaxed);
                                let n: usize = c
                                    .tracks
                                    .iter()
                                    .flat_map(|t| &t.channels)
                                    .map(|ch| ch.keys.len())
                                    .sum();
                                keys.fetch_add(n, Ordering::Relaxed);
                            }
                            Err(err) => failures
                                .lock()
                                .unwrap()
                                .push(format!("{} clip {i}: {err}", e.clean_path())),
                        }
                    }
                }
                Err(err) => failures
                    .lock()
                    .unwrap()
                    .push(format!("{}: {err}", e.clean_path())),
            }
        }
    });
    let failures = failures.into_inner().unwrap();
    println!(
        "mtar archives: {}, clips decoded: {}, keyframes: {}, failures: {}",
        archives.into_inner(),
        clips.into_inner(),
        keys.into_inner(),
        failures.len()
    );
    for f in &failures {
        println!("  {f}");
    }
    Ok(())
}

fn fmdl_check(vfs: &Vfs) -> Result<()> {
    let pkgs: Vec<_> = vfs
        .iter()
        .filter(|(h, _)| fox_hash::extension_from_id((h >> 51) as u16) == Some("fpk"))
        .collect();
    let files = AtomicUsize::new(0);
    let meshes = AtomicUsize::new(0);
    let tris = AtomicUsize::new(0);
    let failures = std::sync::Mutex::new(Vec::new());
    pkgs.par_iter().for_each(|(_, r)| {
        let Ok(data) = vfs.read_ref(*r) else { return };
        let Ok(pkg) = Fpk::parse(&data) else { return };
        for e in pkg
            .entries
            .iter()
            .filter(|e| e.clean_path().ends_with(".fmdl"))
        {
            files.fetch_add(1, Ordering::Relaxed);
            let res = pkg
                .read(e)
                .map_err(anyhow::Error::from)
                .and_then(|d| Ok(fox_data::Fmdl::parse(&d)?))
                .and_then(|m| {
                    m.validate()?;
                    Ok(m)
                });
            match res {
                Ok(m) => {
                    meshes.fetch_add(m.meshes.len(), Ordering::Relaxed);
                    tris.fetch_add(
                        m.meshes.iter().map(|x| x.indices.len() / 3).sum(),
                        Ordering::Relaxed,
                    );
                }
                Err(err) => failures
                    .lock()
                    .unwrap()
                    .push(format!("{}: {err:#}", e.clean_path())),
            }
        }
    });
    let failures = failures.into_inner().unwrap();
    println!(
        "fmdl files: {}, meshes: {}, triangles: {}, failures: {}",
        files.into_inner(),
        meshes.into_inner(),
        tris.into_inner(),
        failures.len()
    );
    for f in failures.iter().take(20) {
        println!("  {f}");
    }
    Ok(())
}

fn fox2_check(vfs: &Vfs) -> Result<()> {
    let names = fox_names(vfs);
    let pkgs: Vec<_> = vfs
        .iter()
        .filter(|(h, _)| fox_hash::extension_from_id((h >> 51) as u16) == Some("fpkd"))
        .collect();
    let files = AtomicUsize::new(0);
    let entities = AtomicUsize::new(0);
    let classes_named = AtomicUsize::new(0);
    let failures = std::sync::Mutex::new(Vec::new());
    pkgs.par_iter().for_each(|(_, r)| {
        let Ok(data) = vfs.read_ref(*r) else { return };
        let Ok(pkg) = Fpk::parse(&data) else { return };
        for e in pkg
            .entries
            .iter()
            .filter(|e| e.clean_path().ends_with(".fox2"))
        {
            files.fetch_add(1, Ordering::Relaxed);
            match pkg
                .read(e)
                .map_err(anyhow::Error::from)
                .and_then(|d| Ok(fox_data::DataSet::parse(&d)?))
            {
                Ok(ds) => {
                    entities.fetch_add(ds.entities.len(), Ordering::Relaxed);
                    let n = ds
                        .entities
                        .iter()
                        .filter(|x| names.get(x.class).is_some())
                        .count();
                    classes_named.fetch_add(n, Ordering::Relaxed);
                }
                Err(err) => failures
                    .lock()
                    .unwrap()
                    .push(format!("{}: {err:#}", e.clean_path())),
            }
        }
    });
    let failures = failures.into_inner().unwrap();
    println!(
        "fox2 files: {}, entities: {}, class names resolved: {}, failures: {}",
        files.into_inner(),
        entities.load(Ordering::Relaxed),
        classes_named.into_inner(),
        failures.len()
    );
    for f in failures.iter().take(20) {
        println!("  {f}");
    }
    Ok(())
}

fn new_host(vfs: Vfs) -> Result<fox_lua::FoxLua> {
    fox_lua::FoxLua::new(std::sync::Arc::new(vfs)).map_err(|e| anyhow::anyhow!("{e}"))
}

fn boot(vfs: Vfs, all: bool) -> Result<()> {
    let host = new_host(vfs)?;
    let mut game = fox_lua::game::Game::new(&host);
    let res = game.boot();
    for (f, e) in &game.events {
        println!("@{f:<6} {e}");
    }
    if let Err(e) = res {
        println!("FAIL  {e}");
    }
    report(&host, all);
    Ok(())
}

fn run_game(
    vfs: Vfs,
    frames: usize,
    mission_code: i64,
    watch: &[String],
    all: bool,
    post: Vec<fox_lua::game::ScheduledMessage>,
    log_messages: bool,
) -> Result<()> {
    let host = new_host(vfs)?;
    let mut game = fox_lua::game::Game::new(&host);
    game.scheduled = post;
    game.log_messages = log_messages;

    // Watched expressions; the current sequence is always included.
    let mut exprs = vec!["TppSequence.GetCurrentSequenceName()".to_string()];
    exprs.extend(watch.iter().cloned());
    let eval = |expr: &str| -> String {
        host.lua
            .load(format!(
                "local ok, v = pcall(function() return {expr} end) return ok and tostring(v) or '<error>'"
            ))
            .eval::<String>()
            .unwrap_or_else(|_| "<error>".into())
    };
    let mut last: Vec<String> = Vec::new();
    let mut printed = 0;
    let flush = |game: &fox_lua::game::Game, printed: &mut usize| {
        for (f, e) in &game.events[*printed..] {
            println!("@{f:<6} {e}");
        }
        *printed = game.events.len();
    };

    let result = (|| -> fox_lua::mlua::Result<()> {
        game.boot()?;
        flush(&game, &mut printed);
        game.load_mission_code(mission_code)?;
        flush(&game, &mut printed);
        last = exprs.iter().map(|e| eval(e)).collect();
        for (e, v) in exprs.iter().zip(&last) {
            println!("@{:<6} {e} = {v}", host.clock.lock().unwrap().frame);
        }
        for _ in 0..frames {
            game.frame()?;
            flush(&game, &mut printed);
            let f = host.clock.lock().unwrap().frame;
            for (i, e) in exprs.iter().enumerate() {
                let now = eval(e);
                if now != last[i] {
                    println!("@{f:<6} {e} = {now}");
                    last[i] = now;
                }
            }
        }
        Ok(())
    })();
    flush(&game, &mut printed);
    match result {
        Ok(()) => println!("ok    ran {frames} frames"),
        Err(e) => println!("FAIL  at frame {}\n{e}", host.clock.lock().unwrap().frame),
    }
    report(&host, all);
    Ok(())
}

/// Print the native-API trace collected by the host.
fn report(host: &fox_lua::FoxLua, all: bool) {
    let t = host.trace.lock().unwrap();
    println!("\nscripts loaded: {}", t.loaded.len());
    for p in &t.missing {
        println!("  missing: {p}");
    }
    println!("implemented natives called: {}", t.native_calls.len());
    for (k, n) in &t.native_calls {
        println!("  {n:>5}  {k}");
    }
    let total: u64 = t.phantom_calls.values().sum();
    println!(
        "unimplemented natives called: {} distinct, {} calls",
        t.phantom_calls.len(),
        total
    );
    let mut v: Vec<_> = t.phantom_calls.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in v.iter().take(if all { usize::MAX } else { 25 }) {
        println!("  {n:>5}  {k}");
    }
    println!(
        "unrecovered native constants used as numbers (read as 0): {}",
        t.phantom_values.len()
    );
    let mut pv: Vec<_> = t.phantom_values.iter().collect();
    pv.sort_by(|a, b| b.1.cmp(a.1));
    for (k, n) in pv.iter().take(if all { usize::MAX } else { 15 }) {
        println!("  {n:>5}  {k}");
    }
    if !t.log.is_empty() {
        println!("script log ({} lines, first 10):", t.log.len());
        for l in t.log.iter().take(10) {
            println!("  {l}");
        }
    }
}

fn info(vfs: &Vfs) -> Result<()> {
    println!("game: {}", vfs.game_dir.display());
    for a in &vfs.archives {
        let rel = a.path().strip_prefix(&vfs.game_dir).unwrap_or(a.path());
        let enc = a.entries.iter().filter(|e| e.encryption != 0).count();
        println!(
            "  {:<45} v{} flags {:#06x} {:>7} entries ({} layer-2)",
            rel.display(),
            a.version,
            a.flags,
            a.entries.len(),
            enc
        );
    }
    let total = vfs.len();
    let named = vfs
        .iter()
        .filter(|(h, _)| vfs.dictionary.resolve(*h).1)
        .count();
    println!(
        "unique files: {total}, names resolved: {named} ({:.1}%), dictionary size: {}",
        100.0 * named as f64 / total.max(1) as f64,
        vfs.dictionary.len()
    );
    let mut by_ext = std::collections::BTreeMap::<&str, usize>::new();
    for (h, _) in vfs.iter() {
        *by_ext
            .entry(fox_hash::extension_from_id((h >> 51) as u16).unwrap_or("?"))
            .or_default() += 1;
    }
    let mut v: Vec<_> = by_ext.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!(
        "by extension: {}",
        v.iter()
            .map(|(e, n)| format!("{e}:{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    Ok(())
}

fn extract(
    vfs: &Vfs,
    filter: Option<String>,
    out: Option<PathBuf>,
    unpack_fpk: bool,
) -> Result<()> {
    let out = out.unwrap_or_else(|| repo_dir().join("extracted"));
    let jobs: Vec<_> = vfs
        .iter()
        .map(|(h, r)| (vfs.dictionary.resolve(h).0, r))
        .filter(|(n, _)| filter.as_ref().is_none_or(|f| n.contains(f.as_str())))
        .collect();
    if jobs.is_empty() {
        bail!("no files match");
    }
    let done = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    jobs.par_iter().for_each(|(name, r)| {
        let dest = out.join(name.trim_start_matches('/'));
        let res = (|| -> Result<()> {
            let data = vfs.read_ref(*r)?;
            write_file(&dest, &data)?;
            if unpack_fpk && fpk::is_fpk(&data) {
                let pkg = Fpk::parse(&data)?;
                let dir = dest.with_file_name(format!(
                    "{}_{}",
                    dest.file_stem().unwrap().to_string_lossy(),
                    dest.extension().unwrap().to_string_lossy()
                ));
                for e in &pkg.entries {
                    write_file(
                        &dir.join(e.clean_path().trim_start_matches('/')),
                        &pkg.read(e)?,
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            failed.fetch_add(1, Ordering::Relaxed);
            eprintln!("{name}: {e:#}");
        }
        let n = done.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 5000 == 0 {
            eprintln!("{n}/{}", jobs.len());
        }
    });
    eprintln!(
        "extracted {} files to {} ({} failed)",
        jobs.len(),
        out.display(),
        failed.load(Ordering::Relaxed)
    );
    Ok(())
}

fn write_file(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(path, data)?;
    Ok(())
}
