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
    /// Dump a .frig rig: units, the bones and track segments they drive,
    /// mask layers (PACKAGE.fpk:INNER).
    Frig { path: String },
    /// Parse every .frig inside every package; one line per distinct rig.
    FrigCheck,
    /// Development probe: print Snake's rest skeleton for the rig bones and
    /// the sampled rig-unit values of one clip at one frame.
    /// Pose Snake with every clip of every player motion archive and report
    /// rig-solve statistics (IK reach, foot heights, root motion).
    AnimCheck {
        /// List clips whose IK targets go far out of reach.
        #[arg(long)]
        verbose: bool,
        /// Compare candidate pole-axis conventions by wrist/ankle bend.
        #[arg(long)]
        poles: bool,
    },
    /// Write a local HTML stick-figure viewer of Snake posed by clips
    /// (default output: extracted/anim_view.html, which stays on this machine).
    AnimView {
        /// .mtar (PACKAGE.fpk:INNER)
        path: String,
        /// Substrings of clip names to include (repeatable; default: first 8 clips).
        #[arg(long)]
        clip: Vec<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    AnimProbe {
        /// .mtar (PACKAGE.fpk:INNER)
        path: String,
        /// Clip index, or a substring of the clip name.
        #[arg(long, default_value = "0")]
        clip: String,
        #[arg(long, default_value_t = 0.0)]
        frame: f32,
        /// Skip the rest-skeleton listing.
        #[arg(long)]
        brief: bool,
        /// Instead of one frame, print root and IK-target tracks per frame.
        #[arg(long)]
        timeline: bool,
    },
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
    for d in ["skl", "rig", "param", "events", "gani", "shader"] {
        let _ = names.load_dictionary(
            repo_dir().join(format!("dictionaries/mtar_tools/{d}_dictionary.txt")),
        );
    }
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
    for name in [
        "qar_dictionary.txt",
        "harvested_paths.txt",
        "mtar_tools/mtar_dictionary.txt",
    ] {
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
            let names = fox_names(&vfs);
            let units: Vec<String> = m
                .layout
                .iter()
                .map(|u| {
                    let name = names
                        .resolve32(u.name)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("{:#010x}", u.name));
                    format!("{name}{:?}", u.segments)
                })
                .collect();
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
        Cmd::Frig { path } => {
            let data = read_any(&vfs, &path)?;
            let r = fox_data::frig::Frig::parse(&data)?;
            let names = fox_names(&vfs);
            let bone = |i: i16| -> String {
                let Some(&(_, h)) = r.bones.get(i as usize) else {
                    return format!("#{i}");
                };
                names
                    .resolve32(h)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{h:#010x}"))
            };
            writeln!(
                out,
                "frig: {} units, {} segments, {} bones, {} mask layers",
                r.units.len(),
                r.segment_count,
                r.bones.len(),
                r.masks.len()
            )?;
            for (i, u) in r.units.iter().enumerate() {
                let bones: Vec<String> = u.bones.iter().map(|&b| bone(b)).collect();
                writeln!(
                    out,
                    "  unit {i:>2} {:<22} parent {:>2} tracks {} segs {:?} bones [{}]{}",
                    format!("{:?}", u.kind),
                    u.parent_unit,
                    u.track_count,
                    u.segments,
                    bones.join(", "),
                    u.plane_normal
                        .map(|n| format!(" normal ({:.2}, {:.2}, {:.2})", n[0], n[1], n[2]))
                        .unwrap_or_default()
                )?;
            }
            for m in &r.masks {
                writeln!(out, "  mask {:<12} {:?}", m.name, m.weights)?;
            }
            Ok(())
        }
        Cmd::FrigCheck => frig_check(&vfs),
        Cmd::AnimProbe {
            path,
            clip,
            frame,
            brief,
            timeline,
        } => anim_probe(&vfs, &path, &clip, frame, brief, timeline),
        Cmd::AnimCheck { verbose, poles } => {
            if poles {
                anim_poles(&vfs)
            } else {
                anim_check(&vfs, verbose)
            }
        }
        Cmd::AnimView { path, clip, out } => anim_view(&vfs, &path, &clip, out),
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

/// Fox human bones are named `SKL_<NNN>_<L|R><PART>`. Try every number
/// with part words taken from known names and identifiers plus common
/// anatomy abbreviations; return the names that hit a target hash.
fn guess_skl(
    known: &std::collections::BTreeSet<String>,
    targets32: &std::collections::HashSet<u32>,
    targets64: &std::collections::HashSet<u64>,
) -> Vec<String> {
    const PARTS: &[&str] = &[
        "WAIST", "HIP", "HIPS", "PELVIS", "SPINE", "SPINE1", "SPINE2", "BELLY", "STOMACH", "CHEST",
        "BREAST", "NECK", "HEAD", "SHLD", "SHOULDER", "CLAV", "CLAVICLE", "COLLAR", "UARM", "FARM",
        "ARM", "UPARM", "FOREARM", "ELBOW", "HAND", "WRIST", "PALM", "LEG", "ULEG", "THIGH",
        "KNEE", "SHIN", "CALF", "FOOT", "ANKLE", "TOE", "TOES", "BALL", "HEEL", "FINGER", "THUMB",
        "INDEX", "MIDDLE", "RING", "PINKY", "LITTLE", "FING", "FNG", "THM", "IDX", "MID", "PNK",
        "PNKY", "RNG", "FA", "FB", "FC", "FD", "FE", "ROLL", "TWIST", "ARMROLL", "UARMROLL",
        "FARMROLL", "SKIRT", "ROOT", "EYE", "JAW", "MOUTH", "DUMMY", "GUN", "IK", "POLE",
    ];
    const SUFFIXES: &[&str] = &[
        "", "1", "2", "3", "4", "5", "A", "B", "C", "D", "E", "1A", "1B", "1C", "2A", "2B", "2C",
        "3A", "3B", "3C", "4A", "4B", "4C", "5A", "5B", "5C", "_A", "_B", "_C", "_1", "_2", "_3",
    ];
    let mut words: std::collections::BTreeSet<String> =
        PARTS.iter().map(|s| s.to_string()).collect();
    for n in known {
        for t in n.split('_') {
            let t = t.to_ascii_uppercase();
            if (2..=10).contains(&t.len()) && t.bytes().all(|b| b.is_ascii_alphanumeric()) {
                let t = t.trim_start_matches(['L', 'R']).to_string();
                if t.len() >= 2 {
                    words.insert(t);
                }
            }
        }
    }
    // Finger joints: F<finger 1-5><joint 0-3>.
    let fingers: Vec<String> = (10..60).map(|n| format!("F{n}")).collect();
    let mut curated: std::collections::HashSet<&str> = PARTS.iter().copied().collect();
    curated.extend(fingers.iter().map(String::as_str));
    words.extend(fingers.iter().cloned());
    let words: Vec<String> = words.into_iter().collect();
    (0..1000u32)
        .into_par_iter()
        .flat_map_iter(|num| {
            let mut out = Vec::new();
            for w in &words {
                let suffixes: &[&str] = if curated.contains(w.as_str()) {
                    SUFFIXES
                } else {
                    &[""]
                };
                for side in ["", "L", "R"] {
                    for sfx in suffixes {
                        let name = format!("SKL_{num:03}_{side}{w}{sfx}");
                        let h = fox_hash::str_code64(name.as_bytes());
                        // A 32-bit (rig) hit is only trusted for curated words:
                        // the large identifier list makes random hits likely.
                        if targets64.contains(&h)
                            || (curated.contains(w.as_str()) && targets32.contains(&(h as u32)))
                        {
                            out.push(name);
                        }
                    }
                }
            }
            out
        })
        .collect()
}

fn bone_names(vfs: &Vfs) -> Result<()> {
    // Candidate names: identifiers in the exe, string tables of models that
    // still carry one (older FMDL versions), and quoted strings in scripts.
    let mut names = std::collections::BTreeSet::new();
    for d in ["skl", "rig"] {
        let path = repo_dir().join(format!("dictionaries/mtar_tools/{d}_dictionary.txt"));
        if let Ok(text) = std::fs::read_to_string(path) {
            names.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_string),
            );
        }
    }
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

    // Bones of the human rig (StrCode32) and Snake's model (StrCode64).
    let rig = fox_data::frig::Frig::parse(&read_any(
        vfs,
        "/Assets/tpp/pack/mission2/common/mis_com_chico_gz.fpk:/Assets/tpp/rig/frig/human_finger.frig",
    )?)?;
    let model = fox_data::Fmdl::parse(&read_any(
        vfs,
        "/Assets/tpp/pack/player/parts/plparts_normal.fpk:/Assets/tpp/chara/sna/Scenes/sna0_main0_def.fmdl",
    )?)?;
    let targets32: std::collections::HashSet<u32> = rig.bones.iter().map(|b| b.1).collect();
    let targets64: std::collections::HashSet<u64> = model
        .bones
        .iter()
        .filter_map(|b| {
            model
                .str_code64s
                .get(b.name as usize)
                .map(|h| h & 0xFFFF_FFFF_FFFF)
        })
        .collect();
    let guessed = guess_skl(&names, &targets32, &targets64);
    println!("{} SKL_ names guessed", guessed.len());
    names.extend(guessed);

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

const HUMAN_RIG: &str =
    "/Assets/tpp/pack/mission2/common/mis_com_chico_gz.fpk:/Assets/tpp/rig/frig/human_finger.frig";
const SNAKE_ARM: &str = "/Assets/tpp/pack/player/fova/plfova_sna0_arm0_v00.fpk:/Assets/tpp/chara/sna/Scenes/sna0_arm0_cov.fmdl";
const SNAKE_FACE: &str = "/Assets/tpp/pack/player/fova/plfova_sna0_face0_v00.fpk:/Assets/tpp/chara/sna/Scenes/sna0_face0_cov.fmdl";
const SNAKE_BODY: &str = "/Assets/tpp/pack/player/parts/plparts_normal.fpk:/Assets/tpp/chara/sna/Scenes/sna0_main0_def.fmdl";

fn snake_rig(vfs: &Vfs) -> Result<(fox_anim::Skeleton, fox_anim::HumanRig)> {
    let rig = fox_data::frig::Frig::parse(&read_any(vfs, HUMAN_RIG)?)?;
    let skel = fox_anim::Skeleton::from_fmdl(&fox_data::Fmdl::parse(&read_any(vfs, SNAKE_BODY)?)?);
    let human = fox_anim::HumanRig::new(&rig, &skel).map_err(anyhow::Error::msg)?;
    Ok((skel, human))
}

/// Every distinct `.mtar` in the player motion packages, as (path, bytes).
fn player_motion_archives(vfs: &Vfs) -> Result<Vec<(String, Vec<u8>)>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (h, r) in vfs.iter() {
        let (name, _) = vfs.dictionary.resolve(h);
        if !name.starts_with("/Assets/tpp/pack/player/motion/") || !name.ends_with(".fpk") {
            continue;
        }
        let data = vfs.read_ref(r)?;
        let pkg = Fpk::parse(&data)?;
        for e in pkg
            .entries
            .iter()
            .filter(|e| e.clean_path().ends_with(".mtar"))
        {
            if seen.insert(e.md5) {
                out.push((format!("{name}:{}", e.clean_path()), pkg.read(e)?));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn anim_view(vfs: &Vfs, path: &str, filters: &[String], out: Option<PathBuf>) -> Result<()> {
    use std::fmt::Write as _;
    let (skel, rig) = snake_rig(vfs)?;
    let names = fox_names(vfs);
    let data = read_any(vfs, path)?;
    let m = fox_data::mtar::Mtar::parse(&data)?;
    let clip_name = |i: usize| vfs.dictionary.resolve(m.entries[i].hash).0;
    let picked: Vec<usize> = if filters.is_empty() {
        (0..m.entries.len().min(8)).collect()
    } else {
        (0..m.entries.len())
            .filter(|&i| filters.iter().any(|f| clip_name(i).contains(f.as_str())))
            .collect()
    };
    let mut json = String::from("{\"bones\":[");
    for (i, b) in skel.bones.iter().enumerate() {
        let n = names
            .resolve32(b.name)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{:#010x}", b.name));
        let parent = b.parent.map_or(-1, |p| p as i64);
        write!(
            json,
            "{}{{\"name\":\"{n}\",\"parent\":{parent}}}",
            if i > 0 { "," } else { "" }
        )?;
    }
    json.push_str("],\"meshes\":[");
    let mut first_mesh = true;
    for (tint, part) in [SNAKE_BODY, SNAKE_FACE, SNAKE_ARM].iter().enumerate() {
        let model = fox_data::Fmdl::parse(&read_any(vfs, part)?)?;
        let skinned = fox_anim::SkinnedModel::bind(&model, &skel).map_err(anyhow::Error::msg)?;
        let (mut lo, mut hi) = (
            fox_anim::Vec3::splat(f32::MAX),
            fox_anim::Vec3::splat(f32::MIN),
        );
        for m in &skinned.meshes {
            let mut inf = Vec::new();
            for (v, i) in m.rest_positions.iter().zip(&m.influences) {
                lo = lo.min(*v);
                hi = hi.max(*v);
                inf.push(i.len().to_string());
                for x in i {
                    inf.push(format!(
                        "{},{:.4},{:.4},{:.4},{:.4}",
                        x.bone, x.weight, x.offset.x, x.offset.y, x.offset.z
                    ));
                }
            }
            let idx: Vec<String> = m.indices.iter().map(u16::to_string).collect();
            write!(
                json,
                "{}{{\"n\":{},\"tint\":{tint},\"inf\":[{}],\"idx\":[{}]}}",
                if first_mesh { "" } else { "," },
                m.rest_positions.len(),
                inf.join(","),
                idx.join(",")
            )?;
            first_mesh = false;
        }
        println!(
            "  {} meshes, {} bones riding on an ancestor, rest bounds {:.2?}..{:.2?}  {}",
            skinned.meshes.len(),
            skinned.unbound_bones,
            lo.to_array(),
            hi.to_array(),
            part.rsplit('/').next().unwrap_or(part)
        );
    }
    json.push_str("],\"clips\":[");
    let mut first_clip = true;
    for &i in &picked {
        let c = m.clip(&data, i)?;
        let name = clip_name(i);
        let short = name
            .rsplit('/')
            .next()
            .unwrap_or(&name)
            .trim_end_matches(".gani");
        let (mut frames, mut rots, mut roots, mut reach) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for f in 0..=c.frame_count {
            let pose = rig.pose(&skel, &c, f as f32).map_err(anyhow::Error::msg)?;
            let p: Vec<String> = pose
                .positions
                .iter()
                .flat_map(|v| [v.x, v.y, v.z])
                .map(|x| format!("{x:.3}"))
                .collect();
            frames.push(format!("[{}]", p.join(",")));
            let q: Vec<String> = pose
                .rotations
                .iter()
                .flat_map(|q| q.to_array())
                .map(|x| format!("{x:.4}"))
                .collect();
            rots.push(format!("[{}]", q.join(",")));
            let (q, t) = rig.root(&c, f as f32);
            roots.push(format!(
                "[{:.4},{:.4},{:.4},{:.4},{:.3},{:.3},{:.3}]",
                q.x, q.y, q.z, q.w, t.x, t.y, t.z
            ));
            let r: Vec<String> = pose.ik_reach.iter().map(|x| format!("{x:.2}")).collect();
            reach.push(format!("[{}]", r.join(",")));
        }
        write!(
            json,
            "{}{{\"name\":\"{short}\",\"frames\":[{}],\"rots\":[{}],\"root\":[{}],\"reach\":[{}]}}",
            if first_clip { "" } else { "," },
            frames.join(","),
            rots.join(","),
            roots.join(","),
            reach.join(",")
        )?;
        first_clip = false;
        println!("  {short} ({} frames)", c.frame_count);
    }
    json.push_str("]}");
    let html = include_str!("anim_view.html").replace("/*DATA*/null", &json);
    let out = out.unwrap_or_else(|| repo_dir().join("extracted/anim_view.html"));
    std::fs::create_dir_all(out.parent().unwrap())?;
    std::fs::write(&out, html)?;
    println!("{} clips -> {}", picked.len(), out.display());
    Ok(())
}

/// For each candidate pole axis, the mean rotation between each IK chain's
/// lower bone and its effector (forearm/hand, shin/foot) over every frame of
/// every player clip. The right convention keeps wrists and ankles natural.
fn anim_poles(vfs: &Vfs) -> Result<()> {
    use fox_anim::Vec3;
    let (skel, mut rig) = snake_rig(vfs)?;
    let archives = player_motion_archives(vfs)?;
    let mut clips = Vec::new();
    for (_, data) in &archives {
        let Ok(m) = fox_data::mtar::Mtar::parse(data) else {
            continue;
        };
        if m.segment_count != 56 {
            continue;
        }
        clips.extend((0..m.entries.len()).filter_map(|i| m.clip(data, i).ok()));
    }
    let joints = rig.ik_joints();
    for (label, axis) in [
        ("+X", Vec3::X),
        ("-X", -Vec3::X),
        ("+Y", Vec3::Y),
        ("-Y", -Vec3::Y),
        ("+Z", Vec3::Z),
        ("-Z", -Vec3::Z),
    ] {
        rig.set_pole_axis(axis);
        let mut sum = vec![0f64; joints.len() * 2];
        let mut n = 0usize;
        for c in &clips {
            for f in (0..=c.frame_count).step_by(3) {
                let pose = rig.pose(&skel, c, f as f32).map_err(anyhow::Error::msg)?;
                for (k, &(lower, eff)) in joints.iter().enumerate() {
                    sum[k] += pose.rotations[lower]
                        .angle_between(pose.rotations[eff])
                        .to_degrees() as f64;
                    // Swing only: where the effector's own axis points vs the lower bone.
                    let d = skel.bones[eff].rest_local.normalize();
                    let swing = (pose.rotations[lower] * d).angle_between(pose.rotations[eff] * d);
                    sum[k + joints.len()] += swing.to_degrees() as f64;
                }
                n += 1;
            }
        }
        let means: Vec<String> = sum
            .iter()
            .map(|s| format!("{:>6.1}", s / n as f64))
            .collect();
        println!(
            "pole axis {label}: mean joint angle (L arm, R arm, L leg, R leg) {} | swing only {}",
            means[..4].join(" "),
            means[4..].join(" ")
        );
    }
    Ok(())
}

fn anim_check(vfs: &Vfs, verbose: bool) -> Result<()> {
    let (skel, rig) = snake_rig(vfs)?;
    let foot = |n: &str| skel.find(fox_hash::str_code32(n.as_bytes())).unwrap();
    let (lfoot, rfoot) = (foot("SKL_032_LFOOT"), foot("SKL_042_RFOOT"));
    let archives = player_motion_archives(vfs)?;
    let (mut clips, mut frames, mut nan, mut stretched, mut skipped) = (0, 0usize, 0, 0usize, 0);
    let mut foot_y = Vec::new();
    let mut reach = vec![Vec::new(); 4];
    for (path, data) in &archives {
        // Old-format archives (facial sets) are not body motion.
        let Ok(m) = fox_data::mtar::Mtar::parse(data) else {
            continue;
        };
        if m.segment_count != 56 {
            continue;
        }
        for i in 0..m.entries.len() {
            let Ok(c) = m.clip(data, i) else { continue };
            clips += 1;
            let mut worst = 0f32;
            let mut worst_frame = 0;
            for f in 0..=c.frame_count {
                let pose = match rig.pose(&skel, &c, f as f32) {
                    Ok(p) => p,
                    Err(e) => {
                        if f == 0 {
                            skipped += 1;
                            println!("  skip {path} clip {i}: {e}");
                        }
                        break;
                    }
                };
                frames += 1;
                if pose.positions.iter().any(|p| !p.is_finite())
                    || pose.rotations.iter().any(|q| !q.is_finite())
                {
                    nan += 1;
                }
                stretched += pose.ik_reach.iter().filter(|&&r| r > 1.02).count();
                for (k, r) in pose.ik_reach.iter().enumerate() {
                    reach[k].push(*r);
                }
                let w = pose.ik_reach.iter().copied().fold(0f32, f32::max);
                if w > worst {
                    worst = w;
                    worst_frame = f;
                }
                foot_y.push(pose.positions[lfoot].y.min(pose.positions[rfoot].y));
            }
            if verbose && worst > 1.5 {
                let (name, _) = vfs.dictionary.resolve(m.entries[i].hash);
                println!(
                    "  reach {worst:>6.2} at frame {worst_frame:>4}/{}  {}  {path}",
                    c.frame_count,
                    name.rsplit('/').next().unwrap_or(&name)
                );
            }
        }
    }
    foot_y.sort_by(f32::total_cmp);
    let pct = |p: f32| foot_y[((foot_y.len() - 1) as f32 * p) as usize];
    println!(
        "{} archives, {clips} clips ({skipped} skipped), {frames} frames posed; non-finite poses: {nan}; IK targets >2% out of reach: {stretched} of {}",
        archives.len(),
        frames * 4
    );
    for (k, r) in reach.iter_mut().enumerate() {
        r.sort_by(f32::total_cmp);
        let q = |p: f32| r[((r.len() - 1) as f32 * p) as usize];
        println!(
            "IK chain {k} reach: p1 {:.2} p50 {:.2} p90 {:.2} p99 {:.2}",
            q(0.01),
            q(0.5),
            q(0.9),
            q(0.99)
        );
    }
    println!(
        "lowest foot height: p1 {:.3}  p50 {:.3}  p99 {:.3}",
        pct(0.01),
        pct(0.5),
        pct(0.99)
    );
    Ok(())
}

fn anim_probe(
    vfs: &Vfs,
    path: &str,
    clip: &str,
    frame: f32,
    brief: bool,
    timeline: bool,
) -> Result<()> {
    use fox_anim::{Quat, Vec3};
    let names = fox_names(vfs);
    let name = |h: u32| {
        names
            .resolve32(h)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{h:#010x}"))
    };
    let rig = fox_data::frig::Frig::parse(&read_any(vfs, HUMAN_RIG)?)?;
    let skel = fox_anim::Skeleton::from_fmdl(&fox_data::Fmdl::parse(&read_any(vfs, SNAKE_BODY)?)?);
    if !brief {
        println!("rig bone list:");
    }
    for (i, &(ri, h)) in rig.bones.iter().enumerate().filter(|_| !brief) {
        let s = skel.find(h);
        let rest = s.map(|s| {
            let b = &skel.bones[s];
            format!(
                "skel {s:>3} parent {:>3?} local {:>6.3?} model {:>6.3?}",
                b.parent,
                b.rest_local.to_array(),
                b.rest_model.to_array()
            )
        });
        println!(
            "  {i:>2} rig {ri:>3} {:<16} {}",
            name(h),
            rest.unwrap_or_default()
        );
    }
    let data = read_any(vfs, path)?;
    let m = fox_data::mtar::Mtar::parse(&data)?;
    let clip = match clip.parse::<usize>() {
        Ok(i) => i,
        Err(_) => (0..m.entries.len())
            .find(|&i| vfs.dictionary.resolve(m.entries[i].hash).0.contains(clip))
            .with_context(|| format!("no clip matching {clip}"))?,
    };
    let c = m.clip(&data, clip)?;
    if timeline {
        let ch: Vec<&fox_data::mtar::Channel> = c.tracks.iter().flat_map(|t| &t.channels).collect();
        let v3 = |s: usize, f: u32| {
            let a = fox_anim::sample::sample(ch[s], f as f32);
            format!("({:>6.3},{:>6.3},{:>6.3})", a[0], a[1], a[2])
        };
        let yaw = |s: usize, f: u32| {
            let q = Quat::from_array(fox_anim::sample::sample(ch[s], f as f32));
            let z = q * Vec3::Z;
            z.x.atan2(z.z).to_degrees()
        };
        println!(
            "frame  root_dq_yaw root_dv                 waist_yaw waist_pos               Lhand                   Lfoot                   Rfoot"
        );
        for f in 0..=c.frame_count {
            println!(
                "{f:>4} {:>8.2} {}  {:>8.2} {} {} {} {}",
                yaw(0, f),
                v3(1, f),
                yaw(2, f),
                v3(3, f),
                v3(9, f),
                v3(16, f),
                v3(19, f)
            );
        }
        return Ok(());
    }
    println!(
        "clip {clip}: {} ({} frames) at frame {frame}",
        vfs.dictionary.resolve(m.entries[clip].hash).0,
        c.frame_count
    );
    for (u, t) in c.tracks.iter().enumerate() {
        println!("  unit {u:>2} {:<20} flags {}", name(t.name), t.flags);
        for ch in &t.channels {
            let v = fox_anim::sample::sample(ch, frame);
            if matches!(
                ch.kind,
                fox_data::mtar::SegmentType::Quat | fox_data::mtar::SegmentType::QuatDiff
            ) {
                let q = Quat::from_array(v);
                let ax = |a: Vec3| {
                    let r = q * a;
                    format!("({:>5.2},{:>5.2},{:>5.2})", r.x, r.y, r.z)
                };
                println!(
                    "      {:?} q {:>6.3?}  X->{} Y->{} Z->{}",
                    ch.kind,
                    v,
                    ax(Vec3::X),
                    ax(Vec3::Y),
                    ax(Vec3::Z)
                );
            } else {
                println!("      {:?} {:>7.3?}", ch.kind, v);
            }
        }
    }
    let human = fox_anim::HumanRig::new(&rig, &skel).map_err(anyhow::Error::msg)?;
    let pose = human.pose(&skel, &c, frame).map_err(anyhow::Error::msg)?;
    println!("pose (IK reach {:.2?}):", pose.ik_reach);
    for (i, &(_, h)) in rig.bones.iter().enumerate().take(21) {
        let s = skel.find(h).unwrap();
        println!(
            "  {i:>2} {:<16} pos {:>6.3?}",
            name(h),
            pose.positions[s].to_array()
        );
    }
    let (r, t) = human.root_motion(&c, 0.0, frame);
    println!(
        "root motion 0..{frame}: rot {:>6.3?} trans {:>6.3?}",
        r.to_array(),
        t.to_array()
    );
    Ok(())
}

fn frig_check(vfs: &Vfs) -> Result<()> {
    let pkgs: Vec<_> = vfs
        .iter()
        .filter(|(h, _)| {
            matches!(
                fox_hash::extension_from_id((h >> 51) as u16),
                Some("fpk" | "fpkd")
            )
        })
        .collect();
    let seen = std::sync::Mutex::new(std::collections::HashSet::new());
    let rows = std::sync::Mutex::new(Vec::new());
    let failures = std::sync::Mutex::new(Vec::new());
    pkgs.par_iter().for_each(|(h, r)| {
        let Ok(data) = vfs.read_ref(*r) else { return };
        let Ok(pkg) = Fpk::parse(&data) else { return };
        for e in pkg
            .entries
            .iter()
            .filter(|e| e.clean_path().ends_with(".frig"))
        {
            if !seen.lock().unwrap().insert(e.md5) {
                continue;
            }
            let pkg_name = vfs.dictionary.resolve(*h).0;
            let res = pkg
                .read(e)
                .map_err(anyhow::Error::from)
                .and_then(|d| Ok(fox_data::frig::Frig::parse(&d)?));
            match res {
                Ok(f) => {
                    let kinds: Vec<String> =
                        f.units.iter().map(|u| format!("{:?}", u.kind)).collect();
                    rows.lock().unwrap().push(format!(
                        "{:>3} units {:>4} segs {:>4} bones  {}:{}  [{}]",
                        f.units.len(),
                        f.segment_count,
                        f.bones.len(),
                        pkg_name,
                        e.clean_path(),
                        kinds.join(" ")
                    ));
                }
                Err(err) => failures
                    .lock()
                    .unwrap()
                    .push(format!("{}: {err:#}", e.clean_path())),
            }
        }
    });
    let mut rows = rows.into_inner().unwrap();
    rows.sort();
    let failures = failures.into_inner().unwrap();
    println!(
        "distinct frig files: {}, failures: {}",
        rows.len() + failures.len(),
        failures.len()
    );
    for r in &rows {
        println!("{r}");
    }
    for f in &failures {
        println!("  FAILED {f}");
    }
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
