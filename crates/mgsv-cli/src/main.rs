use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use fox_archive::{Fpk, Vfs, find_game_dir, fpk};
use rayon::prelude::*;

mod play;
mod raster;

use snake_runtime::{
    HUMAN_RIG, RIFLE_CNP, SNAKE_ARM, SNAKE_BODY, SNAKE_CNP, SNAKE_FACE, cnp, player_clip_library,
    read_any, snake_rig,
};

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
    /// Print one file to stdout (PACKAGE.fpk:INNER for files inside packages).
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
    /// List an .fmdl's materials and textures, with the archive entries of
    /// each texture (`.ftex` and its `N.ftexs` parts).
    FmdlTex {
        path: String,
        /// Hex-dump each .ftex header and the start of each .ftexs part.
        #[arg(long)]
        dump: bool,
        /// Also write each texture's largest mip as a PNG into this folder.
        #[arg(long)]
        png: Option<PathBuf>,
    },
    /// Hex-dump the blocks of a NIF from a Skyrim install's BSAs (or a
    /// loose file path).
    SkyrimNif {
        #[arg(long)]
        skyrim: PathBuf,
        path: String,
        /// Also write the file's bytes here.
        #[arg(long)]
        save: Option<PathBuf>,
    },
    /// Export Snake's weapons (meshes and textures, from this MGSV install)
    /// into a Skyrim SE install's Data folder for the SnakeSKSE plugin.
    SkyrimExport {
        #[arg(long)]
        skyrim: PathBuf,
    },
    /// Print StrCode32/StrCode64, PathCode64 and Wwise FNV-1 hashes of each TEXT.
    Hash { text: Vec<String> },
    /// Write a 16-bit PCM sound from an .sbp sound package as a .wav.
    SoundExport {
        /// Archive path of the .sbp (e.g. /Assets/tpp/sound/asset/common_bank_01.sbp).
        #[arg(long)]
        bank: String,
        /// Sound (wem) id inside the bank.
        #[arg(long)]
        wem: u32,
        #[arg(long)]
        out: PathBuf,
    },
    /// Decode a texture (archive path of the .ftex) to a .png (largest mip).
    TexPng {
        path: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// List files in a Skyrim install's BSAs whose path contains FILTER.
    SkyrimLs {
        #[arg(long)]
        skyrim: PathBuf,
        filter: String,
    },
    /// List records of a Skyrim plugin (default Skyrim.esm) whose editor ID
    /// contains FILTER (case-insensitive), optionally of one TYPE (e.g. ALCH).
    SkyrimForms {
        #[arg(long)]
        skyrim: PathBuf,
        #[arg(long, default_value = "Skyrim.esm")]
        plugin: String,
        #[arg(long = "type")]
        kind: Option<String>,
        filter: String,
    },
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
    /// Catalogue every player body clip: archive, frames, root travel,
    /// speed and turn (tab-separated).
    AnimClips,
    /// Dump an .fcnp connection-point file (PACKAGE.fpk:INNER).
    Fcnp { path: String },
    /// Parse every .fcnp inside every package.
    FcnpCheck,
    /// Open a window and drive Snake around a test floor (keyboard + mouse).
    Play {
        /// Quit after this many frames (smoke tests).
        #[arg(long)]
        frames: Option<usize>,
        /// Save the last frame as a PNG.
        #[arg(long)]
        shot: Option<PathBuf>,
        /// Scripted input instead of the keyboard.
        #[arg(long)]
        auto: bool,
    },
    /// Dump Skyrim's character skeleton (from Skyrim - Meshes0.bsa): node
    /// tree with rest transforms and model-space positions.
    SkyrimSkeleton {
        /// Skyrim Special Edition folder.
        #[arg(long)]
        skyrim: PathBuf,
        /// Path inside the archives.
        #[arg(
            long,
            default_value = "meshes\\actors\\character\\character assets\\skeleton.nif"
        )]
        nif: String,
    },
    /// Retarget Snake's poses onto Skyrim's skeleton and report how well limb
    /// directions match (degrees) over a set of clips.
    SkyrimRetargetCheck {
        #[arg(long)]
        skyrim: PathBuf,
    },
    /// Run the player controller headless with a scripted input sequence and
    /// print position, speed and clip over time.
    SnakeSim,
    /// Development probe: for each frame of a resident clip, the chest and
    /// right-hand direction (yaw/pitch, degrees) and hand position.
    AimProbe { clip: String },
    /// Development probe: attach the rifle to the right hand and report,
    /// per frame of a clip, the muzzle direction and how far the rifle's
    /// left-hand grip is from the animated left hand.
    WeaponProbe { clip: String },
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

/// Names for .fox2 hashes: FoxTool's dictionary plus identifiers harvested
/// from the user's own mgsvtpp.exe.
/// Material texture slot names used by TPP shaders (matched by hash).
const TEXTURE_SLOTS: &[&str] = &[
    "Base_Tex_SRGB",
    "Base_Tex_LIN",
    "NormalMap_Tex_NRM",
    "SpecularMap_Tex_LIN",
    "SpecularMap_Tex_SRGB",
    "Translucent_Tex_LIN",
    "Layer_Tex_SRGB",
    "LayerMask_Tex_LIN",
    "Dirty_Tex_LIN",
    "Dirty_Tex_SRGB",
    "MatParamMap_Tex_LIN",
    "SubNormalMap_Tex_NRM",
    "CubeMap_Tex_LIN",
    "Emissive_Tex_SRGB",
    "Incidence_Tex_LIN",
    "RoughnessMap_Tex_LIN",
];

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
    snake_runtime::open_game(&dir, &repo_dir().join("dictionaries"))
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
            // Unnamed entries as `ls` prints them: `<hex path hash>.<ext>`.
            let unnamed = path.split_once('.').and_then(|(stem, ext)| {
                let hash = u64::from_str_radix(stem, 16).ok()?;
                let id = ((fox_hash::extension_id(ext) as u64) << 51) | hash;
                vfs.lookup_hash(id)
            });
            match unnamed {
                Some(r) => out.write_all(&vfs.read_ref(r)?)?,
                None => out.write_all(&read_any(&vfs, &path)?)?,
            }
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
        Cmd::FmdlTex { path, dump, png } => {
            let m = fox_data::Fmdl::parse(&read_any(&vfs, &path)?)?;
            let names = fox_names(&vfs);
            for (i, mi) in m.material_instances.iter().enumerate() {
                writeln!(
                    out,
                    "material {i} {} ({}) textures {:?} parameters {:?}",
                    m.name(mi.name, &names),
                    m.name(m.materials[mi.material as usize].name, &names),
                    mi.textures,
                    mi.parameters
                )?;
                // A material's texture slots are entries of the parameter
                // table whose reference indexes the texture table.
                for slot in m.material_parameters.get(mi.textures.clone()).unwrap_or_default() {
                    let Some(t) = m.textures.get(slot.reference as usize) else {
                        continue;
                    };
                    let pc = m.path_code64s.get(t.reference as usize).copied().unwrap_or(0);
                    let stem = pc & ((1u64 << 51) - 1);
                    let mut parts = Vec::new();
                    for ext in ["ftex", "1.ftexs", "2.ftexs", "3.ftexs", "4.ftexs", "5.ftexs", "6.ftexs"] {
                        let id = ((fox_hash::extension_id(ext) as u64) << 51) | stem;
                        if let Some(r) = vfs.lookup_hash(id) {
                            let data = vfs.read_ref(r).unwrap_or_default();
                            parts.push(format!("{ext} {}", data.len()));
                            if dump {
                                let n = if ext == "ftex" { data.len() } else { 48.min(data.len()) };
                                for (row, chunk) in data[..n].chunks(16).enumerate() {
                                    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                                    writeln!(out, "    {ext:>8} {:04x}: {}", row * 16, hex.join(" "))?;
                                }
                            }
                        }
                    }
                    let slot_name = m.name(slot.name, &names);
                    let slot_name = TEXTURE_SLOTS
                        .iter()
                        .find(|c| format!("{:#x}", fox_hash::str_code64(c.as_bytes()) & 0xFFFF_FFFF_FFFF) == slot_name)
                        .map(|c| c.to_string())
                        .unwrap_or(slot_name);
                    writeln!(out, "  {slot_name:<24} {pc:#018x}  {}", parts.join(", "))?;
                    if dump {
                        for (h, _) in vfs.iter().filter(|(h, _)| h & ((1u64 << 51) - 1) == stem) {
                            let ext = fox_hash::extension_from_id((h >> 51) as u16);
                            writeln!(out, "      entry {h:#018x} ext {ext:?}")?;
                        }
                    }
                    // Decode the largest mip and report per-channel means.
                    let read = |ext: String| {
                        vfs.lookup_hash(((fox_hash::extension_id(&ext) as u64) << 51) | stem)
                            .and_then(|r| vfs.read_ref(r).ok())
                    };
                    if let Some(header) = read("ftex".into()) {
                        let f = fox_data::Ftex::parse(&header)?;
                        let mips = f.read_mips(|n| read(format!("{n}.ftexs")))?;
                        let mut info = format!("{:?} {}x{} v{:.2}, {} of {} mips", f.format, f.width, f.height, f.version, mips.len(), f.mips.len());
                        if let Some(m0) = mips.first() {
                            let px = fox_data::ftex::decode_rgba(f.format, f.width, f.height, m0)?;
                            if let Some(dir) = &png {
                                std::fs::create_dir_all(dir)?;
                                let (w, h) = (f.width as usize, f.height as usize);
                                let mut r = raster::Raster::new(w * 2, h);
                                for (k, p) in px.iter().enumerate() {
                                    let (x, y) = (k % w, k / w);
                                    r.color[y * w * 2 + x] = ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32;
                                    let a = p[3] as u32;
                                    r.color[y * w * 2 + w + x] = (a << 16) | (a << 8) | a;
                                }
                                raster::save_png(&r, &dir.join(format!("m{i}_{slot_name}.png")))?;
                            }
                            let mut sum = [0f64; 4];
                            for p in &px {
                                for k in 0..4 {
                                    sum[k] += p[k] as f64;
                                }
                            }
                            let n = px.len() as f64;
                            info += &format!(", mean rgba {:.0} {:.0} {:.0} {:.0}", sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n);
                        }
                        writeln!(out, "      {info}")?;
                    }
                }
            }
            Ok(())
        }
        Cmd::SkyrimNif { skyrim, path, save } => {
            let data = match std::fs::read(&path) {
                Ok(d) => d,
                Err(_) => skyrim_data::read_install_file(&skyrim, &path)?,
            };
            if let Some(f) = &save {
                std::fs::write(f, &data)?;
            }
            let nif = skyrim_data::Nif::parse(&data)?;
            writeln!(out, "user version {}, bs version {}, strings {:?}", nif.user_version, nif.bs_version, nif.strings)?;
            for (i, (kind, range)) in nif.blocks.iter().enumerate() {
                writeln!(out, "block {i} {kind} ({} bytes)", range.len())?;
                for (row, chunk) in data[range.clone()].chunks(16).enumerate().take(12) {
                    let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                    writeln!(out, "  {:04x}: {}", row * 16, hex.join(" "))?;
                }
            }
            Ok(())
        }
        Cmd::SkyrimExport { skyrim } => {
            let report = snake_runtime::skyrim_export::export_weapons(&vfs, &skyrim.join("Data"))?;
            for f in &report.files {
                writeln!(out, "wrote {} ({} bytes)", f.display(), std::fs::metadata(f).map(|m| m.len()).unwrap_or(0))?;
            }
            for n in &report.notes {
                writeln!(out, "note: {n}")?;
            }
            Ok(())
        }
        Cmd::Hash { text } => {
            for t in &text {
                let fnv = t.to_ascii_lowercase().bytes().fold(2166136261u32, |h, b| h.wrapping_mul(16777619) ^ b as u32);
                writeln!(
                    out,
                    "{t}: str32 {:#010x} str64 {:#014x} path64 {:#018x} fnv {fnv:#010x}",
                    fox_hash::str_code32(t.as_bytes()),
                    fox_hash::str_code64(t.as_bytes()),
                    fox_hash::path_code64(t)
                )?;
            }
            Ok(())
        }
        Cmd::SoundExport { bank, wem, out: path } => {
            snake_runtime::skyrim_export::export_sound(&vfs, &bank, wem, &path)?;
            writeln!(out, "wrote {}", path.display())?;
            Ok(())
        }
        Cmd::TexPng { path, out: png } => {
            let read = |p: &str| vfs.read(p).ok();
            let header = read(&path).with_context(|| format!("{path} not found"))?;
            let f = fox_data::Ftex::parse(&header)?;
            let stem = path.trim_end_matches(".ftex");
            let mips = f.read_mips(|n| read(&format!("{stem}.{n}.ftexs")))?;
            let m0 = mips.first().context("no mips")?;
            let px = fox_data::ftex::decode_rgba(f.format, f.width, f.height, m0)?;
            // Left: colour over black weighted by alpha; right: alpha.
            let (w, h) = (f.width as usize, f.height as usize);
            let mut r = raster::Raster::new(w * 2, h);
            for y in 0..h {
                for x in 0..w {
                    let p = px[y * w + x];
                    let a = p[3] as u32;
                    let c = |v: u8| v as u32 * a / 255;
                    r.color[y * w * 2 + x] = (c(p[0]) << 16) | (c(p[1]) << 8) | c(p[2]);
                    r.color[y * w * 2 + w + x] = (a << 16) | (a << 8) | a;
                }
            }
            raster::save_png(&r, &png)?;
            writeln!(out, "{:?} {}x{}, {} mips -> {}", f.format, f.width, f.height, mips.len(), png.display())?;
            Ok(())
        }
        Cmd::SkyrimLs { skyrim, filter } => {
            let filter = filter.to_ascii_lowercase();
            let mut bsas: Vec<_> = std::fs::read_dir(skyrim.join("Data"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bsa")))
                .collect();
            bsas.sort();
            for b in bsas {
                let Ok(bsa) = skyrim_data::Bsa::open(&b) else { continue };
                for e in bsa.entries.iter().filter(|e| e.path.contains(&filter)) {
                    writeln!(out, "{}  ({})", e.path, b.file_name().unwrap_or_default().to_string_lossy())?;
                }
            }
            Ok(())
        }
        Cmd::SkyrimForms { skyrim, plugin, kind, filter } => {
            let data = std::fs::read(skyrim.join("Data").join(&plugin))?;
            let want: Option<[u8; 4]> = kind.as_deref().and_then(|k| k.as_bytes().try_into().ok());
            let types: Vec<&[u8; 4]> = want.iter().collect();
            let filter = filter.to_ascii_lowercase();
            for r in skyrim_data::esm::records(&data, &types)? {
                if r.editor_id.to_ascii_lowercase().contains(&filter) {
                    writeln!(out, "{} {:08X} {}", String::from_utf8_lossy(&r.kind), r.form_id, r.editor_id)?;
                }
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
        Cmd::AnimClips => anim_clips(&vfs),
        Cmd::SnakeSim => snake_sim(&vfs),
        Cmd::SkyrimSkeleton { skyrim, nif } => skyrim_skeleton(&skyrim, &nif),
        Cmd::SkyrimRetargetCheck { skyrim } => skyrim_retarget_check(&vfs, &skyrim),
        Cmd::Fcnp { path } => {
            let f = fox_data::fcnp::Fcnp::parse(&read_any(&vfs, &path)?)?;
            writeln!(out, "{}: {} connection points", f.model, f.points.len())?;
            for c in &f.points {
                writeln!(
                    out,
                    "  {:<24} pos {:>7.3?} rot {:>6.3?} {:?}",
                    c.name, c.position, c.rotation, c.params
                )?;
            }
            Ok(())
        }
        Cmd::FcnpCheck => {
            let pkgs: Vec<_> = vfs
                .iter()
                .filter(|(h, _)| fox_hash::extension_from_id((h >> 51) as u16) == Some("fpk"))
                .collect();
            let seen = std::sync::Mutex::new(std::collections::HashSet::new());
            let (ok, points) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let failures = std::sync::Mutex::new(Vec::new());
            pkgs.par_iter().for_each(|(_, r)| {
                let Ok(data) = vfs.read_ref(*r) else { return };
                let Ok(pkg) = Fpk::parse(&data) else { return };
                for e in pkg
                    .entries
                    .iter()
                    .filter(|e| e.clean_path().ends_with(".fcnp"))
                {
                    if !seen.lock().unwrap().insert(e.md5) {
                        continue;
                    }
                    match pkg
                        .read(e)
                        .map_err(anyhow::Error::from)
                        .and_then(|d| Ok(fox_data::fcnp::Fcnp::parse(&d)?))
                    {
                        Ok(f) => {
                            ok.fetch_add(1, Ordering::Relaxed);
                            points.fetch_add(f.points.len(), Ordering::Relaxed);
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
                "fcnp files: {} ok, {} connection points, {} failures",
                ok.into_inner(),
                points.into_inner(),
                failures.len()
            );
            for f in failures.iter().take(20) {
                println!("  {f}");
            }
            Ok(())
        }
        Cmd::AimProbe { clip } => aim_probe(&vfs, &clip),
        Cmd::WeaponProbe { clip } => weapon_probe(&vfs, &clip),
        Cmd::Play { frames, shot, auto } => {
            let assets = snake_runtime::SnakeAssets::load(&vfs)?;
            for w in &assets.weapons {
                println!("equipped {:?}", w.stats);
                for (slot, g) in w.rig.grids.iter().enumerate() {
                    if let Some(g) = g {
                        let deg = |v: &[f32]| {
                            v.iter()
                                .map(|a| format!("{:.0}", a.to_degrees()))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        println!(
                            "  grid {slot}: {} x {}, yaw [{}], pitch [{}]",
                            g.cols,
                            g.rows,
                            deg(&g.col_yaw),
                            deg(&g.row_pitch)
                        );
                    }
                }
            }
            play::run(assets, play::Options { frames, shot, auto })
        }
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

fn weapon_probe(vfs: &Vfs, name: &str) -> Result<()> {
    use fox_anim::Vec3;
    let (skel, rig) = snake_rig(vfs)?;
    let clips = player_clip_library(vfs)?;
    let clip = clips.get(name).with_context(|| format!("no clip {name}"))?;
    let body = fox_data::fcnp::Fcnp::parse(&read_any(vfs, SNAKE_CNP)?)?;
    let gun = fox_data::fcnp::Fcnp::parse(&read_any(vfs, RIFLE_CNP)?)?;
    let (rh_rot, rh_pos) = cnp(&body, "CNP_RIGHT_HAND")?;
    let (lh_rot, lh_pos) = cnp(&body, "CNP_LEFT_HAND")?;
    let (_, grip) = cnp(&gun, "CNP_LEFT_HAND")?;
    let (_, muzzle) = cnp(&gun, "CNP_MUZZLE_FLASH")?;
    let bone = |n: &str| skel.find(fox_hash::str_code32(n.as_bytes())).unwrap();
    let (rhand, lhand) = (bone("SKL_023_RHAND"), bone("SKL_013_LHAND"));
    let _ = lh_rot;
    // Calibrate the hand-to-gun rotation on the centre aim pose (frame 12 of
    // the 5x5 rifle grid): there the rifle points straight ahead, level.
    let grid = clips
        .get("snapasr_s_fre_pose_25")
        .context("aim grid clip")?;
    let centre = rig.pose(&skel, &grid, 12.0).map_err(anyhow::Error::msg)?;
    let hand_to_gun = (centre.rotations[rhand] * rh_rot).inverse();
    println!("hand-to-gun rotation {:?}", hand_to_gun.to_array());
    let mut total = 0.0;
    for f in 0..=clip.frame_count {
        let pose = rig
            .pose(&skel, &clip, f as f32)
            .map_err(anyhow::Error::msg)?;
        let root_rot = pose.rotations[rhand] * rh_rot * hand_to_gun;
        let root_pos = pose.positions[rhand] + pose.rotations[rhand] * rh_pos;
        let grip_world = root_pos + root_rot * grip;
        let lh_world = pose.positions[lhand] + pose.rotations[lhand] * lh_pos;
        let fwd = root_rot * Vec3::Z;
        let m = root_pos + root_rot * muzzle;
        let d = (grip_world - lh_world).length();
        total += d;
        println!(
            "  {f:>3} muzzle yaw {:>6.1} pitch {:>6.1} at ({:>5.2},{:>5.2},{:>5.2}) | grip-to-left-hand {:.3} m",
            fwd.x.atan2(fwd.z).to_degrees(),
            fwd.y.asin().to_degrees(),
            m.x,
            m.y,
            m.z,
            d
        );
    }
    println!(
        "mean grip-to-left-hand {:.3} m",
        total / (clip.frame_count + 1) as f32
    );
    Ok(())
}

fn aim_probe(vfs: &Vfs, name: &str) -> Result<()> {
    use fox_anim::Vec3;
    let (skel, rig) = snake_rig(vfs)?;
    let clips = player_clip_library(vfs)?;
    let clip = clips.get(name).with_context(|| format!("no clip {name}"))?;
    let bone = |n: &str| skel.find(fox_hash::str_code32(n.as_bytes())).unwrap();
    let (chest, hand, lhand) = (
        bone("SKL_002_CHEST"),
        bone("SKL_023_RHAND"),
        bone("SKL_013_LHAND"),
    );
    let dir = |v: Vec3| (v.x.atan2(v.z).to_degrees(), v.y.asin().to_degrees());
    println!("{name}: {} frames", clip.frame_count);
    for f in 0..=clip.frame_count {
        let pose = rig
            .pose(&skel, &clip, f as f32)
            .map_err(anyhow::Error::msg)?;
        // Chest forward is +Z in its rest frame; the hand points along -X (right hand).
        let (cy, cp) = dir(pose.rotations[chest] * Vec3::Z);
        let (hy, hp) = dir((pose.positions[lhand] - pose.positions[hand]).normalize());
        let h = pose.positions[hand];
        println!(
            "  {f:>3} chest yaw {cy:>6.1} pitch {cp:>6.1} | R->L hand yaw {hy:>6.1} pitch {hp:>6.1} | Rhand ({:>5.2},{:>5.2},{:>5.2})",
            h.x, h.y, h.z
        );
    }
    Ok(())
}

fn skyrim_file(skyrim: &Path, inner: &str) -> Result<Vec<u8>> {
    Ok(skyrim_data::read_install_file(skyrim, inner)?)
}

fn skyrim_retarget_check(vfs: &Vfs, skyrim: &Path) -> Result<()> {
    use fox_anim::Vec3;
    let nif = skyrim_data::Nif::parse(&skyrim_file(
        skyrim,
        "meshes\\actors\\character\\character assets\\skeleton.nif",
    )?)?;
    let assets = snake_runtime::SnakeAssets::load(vfs)?;
    let rt = snake_runtime::skyrim::SkyrimRetarget::new(&nif, &assets.skel)?;
    println!("{} Skyrim bones retargeted", rt.len());
    let fox_to_sky = |v: Vec3| Vec3::new(-v.x, v.z, v.y);
    let snake = |n: &str| {
        assets
            .skel
            .find(fox_hash::str_code32(n.as_bytes()))
            .unwrap()
    };
    // (label, Skyrim from, Skyrim to, Snake from, Snake to)
    let segments = [
        (
            "L upper arm",
            "NPC L UpperArm [LUar]",
            "NPC L Forearm [LLar]",
            "SKL_011_LUARM",
            "SKL_012_LFARM",
        ),
        (
            "L forearm",
            "NPC L Forearm [LLar]",
            "NPC L Hand [LHnd]",
            "SKL_012_LFARM",
            "SKL_013_LHAND",
        ),
        (
            "R upper arm",
            "NPC R UpperArm [RUar]",
            "NPC R Forearm [RLar]",
            "SKL_021_RUARM",
            "SKL_022_RFARM",
        ),
        (
            "R forearm",
            "NPC R Forearm [RLar]",
            "NPC R Hand [RHnd]",
            "SKL_022_RFARM",
            "SKL_023_RHAND",
        ),
        (
            "L thigh",
            "NPC L Thigh [LThg]",
            "NPC L Calf [LClf]",
            "SKL_030_LTHIGH",
            "SKL_031_LLEG",
        ),
        (
            "L shin",
            "NPC L Calf [LClf]",
            "NPC L Foot [Lft ]",
            "SKL_031_LLEG",
            "SKL_032_LFOOT",
        ),
        (
            "R thigh",
            "NPC R Thigh [RThg]",
            "NPC R Calf [RClf]",
            "SKL_040_RTHIGH",
            "SKL_041_RLEG",
        ),
        (
            "R shin",
            "NPC R Calf [RClf]",
            "NPC R Foot [Rft ]",
            "SKL_041_RLEG",
            "SKL_042_RFOOT",
        ),
        (
            "L foot",
            "NPC L Foot [Lft ]",
            "NPC L Toe0 [LToe]",
            "SKL_032_LFOOT",
            "SKL_033_LTOE",
        ),
        (
            "neck",
            "NPC Neck [Neck]",
            "NPC Head [Head]",
            "SKL_003_NECK",
            "SKL_004_HEAD",
        ),
    ];
    let clips = [
        "snapnon_s_idl_l",
        "snapnon_s_rn_lp_vr1_l",
        "snapnon_q_idl_l",
        "snapnon_c_fst_lp_r",
        "snaprdy_s_fre0_idl_0",
        "snapasr_s_fre_pose_25",
    ];
    let mut worst = 0f32;
    for name in clips {
        let clip = assets
            .clips
            .get(name)
            .with_context(|| format!("clip {name}"))?;
        let mut sums = vec![0f32; segments.len()];
        let mut n = 0;
        let mut height = (0f32, 0f32);
        for f in (0..=clip.frame_count).step_by(3) {
            let pose = assets
                .rig
                .pose(&assets.skel, &clip, f as f32)
                .map_err(anyhow::Error::msg)?;
            let joints: std::collections::HashMap<String, Vec3> =
                rt.joint_positions(&pose).into_iter().collect();
            for (k, (_, a, b, sa, sb)) in segments.iter().enumerate() {
                let sky = (joints[*b] - joints[*a]).normalize_or_zero();
                let fox = fox_to_sky(pose.positions[snake(sb)] - pose.positions[snake(sa)])
                    .normalize_or_zero();
                sums[k] += sky.angle_between(fox).to_degrees();
            }
            height = (
                joints["NPC Head [Head]"].z,
                fox_to_sky(pose.positions[snake("SKL_004_HEAD")]).z * 70.0,
            );
            n += 1;
        }
        let means: Vec<String> = sums
            .iter()
            .map(|s| format!("{:.1}", s / n as f32))
            .collect();
        worst = worst.max(sums.iter().map(|s| s / n as f32).fold(0.0, f32::max));
        println!(
            "{name:<24} mean segment angle (deg): {}  | head z sky {:.0} vs snake {:.0}",
            means.join(" "),
            height.0,
            height.1
        );
    }
    println!(
        "segments: {}",
        segments.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")
    );
    println!("worst mean angle {worst:.1} deg");
    Ok(())
}

fn skyrim_skeleton(skyrim: &Path, inner: &str) -> Result<()> {
    use fox_anim::glam::{Mat3, Vec3};
    let nif = skyrim_data::Nif::parse(&skyrim_file(skyrim, inner)?)?;
    println!(
        "{inner}: NIF {:#x} user {} bs {}, {} blocks, {} nodes",
        nif.version,
        nif.user_version,
        nif.bs_version,
        nif.nodes.len(),
        nif.nodes.iter().flatten().count()
    );
    // Model-space transforms under both matrix conventions: `rows` treats the
    // stored rows as the matrix rows (v' = M v), `cols` as its columns.
    fn walk(nif: &skyrim_data::Nif, block: usize, depth: usize, parent: (Mat3, Vec3, Mat3, Vec3)) {
        let Some(n) = nif.node(block) else { return };
        let r = n.rotation;
        let rows = Mat3::from_cols_array_2d(&r).transpose();
        let cols = Mat3::from_cols_array_2d(&r);
        let t = Vec3::from_array(n.translation);
        let (pr, pt, pc, ptc) = parent;
        let (mr, mt) = (pr * rows, pt + pr * t);
        let (mc, mtc) = (pc * cols, ptc + pc * t);
        println!(
            "{:indent$}{} [{}]  local t ({:.1}, {:.1}, {:.1})  model(rows) ({:.1}, {:.1}, {:.1})  model(cols) ({:.1}, {:.1}, {:.1})",
            "",
            n.name,
            n.kind,
            t.x,
            t.y,
            t.z,
            mt.x,
            mt.y,
            mt.z,
            mtc.x,
            mtc.y,
            mtc.z,
            indent = depth * 2
        );
        for &c in &n.children {
            walk(nif, c, depth + 1, (mr, mt, mc, mtc));
        }
    }
    if let Some(root) = nif.root() {
        walk(
            &nif,
            root.block,
            0,
            (Mat3::IDENTITY, Vec3::ZERO, Mat3::IDENTITY, Vec3::ZERO),
        );
    }
    Ok(())
}

fn snake_sim(vfs: &Vfs) -> Result<()> {
    use snake_core::{ClipNames, Controller, Input};
    let (skel, rig) = snake_rig(vfs)?;
    let clips = player_clip_library(vfs)?;
    let mut ctl = Controller::new(&clips, ClipNames::default()).map_err(anyhow::Error::msg)?;
    // (seconds, input, label)
    let fwd = |amount: f32| Input {
        move_y: amount,
        ..Default::default()
    };
    let script: Vec<(f32, Input, &str)> = vec![
        (1.0, Input::default(), "idle"),
        (3.0, fwd(0.4), "walk forward"),
        (3.0, fwd(1.0), "run forward"),
        (
            3.0,
            Input {
                sprint: true,
                ..fwd(1.0)
            },
            "dash",
        ),
        (
            2.0,
            Input {
                move_x: 1.0,
                ..Default::default()
            },
            "run right (turn)",
        ),
        (1.5, Input::default(), "stop"),
        (
            2.0,
            Input {
                move_y: -1.0,
                ..Default::default()
            },
            "start back (180)",
        ),
        (1.5, Input::default(), "stop"),
        (
            2.0,
            Input {
                move_x: -1.0,
                ..Default::default()
            },
            "start left (90)",
        ),
        (1.5, Input::default(), "stop"),
        (
            0.05,
            Input {
                crouch: true,
                ..Default::default()
            },
            "crouch",
        ),
        (2.0, Input::default(), "crouched idle"),
        (3.0, fwd(1.0), "crouch run"),
        (
            0.05,
            Input {
                prone: true,
                ..Default::default()
            },
            "prone",
        ),
        (3.0, Input::default(), "prone idle"),
        (3.0, fwd(1.0), "crawl"),
        (
            0.05,
            Input {
                prone: true,
                ..Default::default()
            },
            "stand up",
        ),
        (3.0, Input::default(), "standing"),
    ];
    let dt = 1.0 / 60.0;
    let mut t = 0.0f32;
    for (secs, input, label) in script {
        let start = ctl.position;
        let steps = (secs / dt).round().max(1.0) as usize;
        for k in 0..steps {
            // Edge-triggered buttons fire on the first tick only.
            let mut i = input;
            if k > 0 {
                i.crouch = false;
                i.prone = false;
            }
            ctl.update(
                &rig,
                &clips,
                &snake_core::FlatGround { height: 0.0 },
                dt,
                &i,
            );
            t += dt;
        }
        let pose = ctl.pose(&rig, &skel).map_err(anyhow::Error::msg)?;
        let head = pose.positions[skel.find(fox_hash::str_code32(b"SKL_004_HEAD")).unwrap()];
        let moved = ctl.position - start;
        println!(
            "t={t:>5.2}s {label:<18} {:>6}/{:<5}  pos ({:>6.2},{:>6.2})  yaw {:>5.0}  avg speed {:>4.2} m/s  head y {:.2}  clip {}",
            format!("{:?}", ctl.stance),
            format!("{:?}", ctl.gait),
            ctl.position.x,
            ctl.position.z,
            ctl.yaw.to_degrees(),
            (moved.x.hypot(moved.z)) / secs,
            head.y,
            ctl.current_clip()
        );
    }
    Ok(())
}

fn anim_clips(vfs: &Vfs) -> Result<()> {
    use fox_anim::Vec3;
    let (_, rig) = snake_rig(vfs)?;
    println!("archive	clip	frames	dx	dz	dist_m	speed_mps	turn_deg	loop");
    for (path, data) in player_motion_archives(vfs)? {
        let Ok(m) = fox_data::mtar::Mtar::parse(&data) else {
            continue;
        };
        if m.segment_count != 56 {
            continue;
        }
        let archive = path.rsplit('/').next().unwrap_or(&path).to_string();
        for i in 0..m.entries.len() {
            let Ok(c) = m.clip(&data, i) else { continue };
            let name = vfs.dictionary.resolve(m.entries[i].hash).0;
            let short = name
                .rsplit('/')
                .next()
                .unwrap_or(&name)
                .trim_end_matches(".gani")
                .to_string();
            let (r, t) = rig.root_motion(&c, 0.0, c.frame_count as f32);
            let fwd = r * Vec3::Z;
            let turn = fwd.x.atan2(fwd.z).to_degrees();
            let dist = Vec3::new(t.x, 0.0, t.z).length();
            let secs = c.frame_count as f32 / 30.0;
            let looping = c
                .tracks
                .iter()
                .any(|t| t.flags & fox_data::mtar::UNIT_LOOP != 0);
            println!(
                "{archive}	{short}	{}	{:.3}	{:.3}	{dist:.3}	{:.2}	{turn:.0}	{}",
                c.frame_count,
                t.x,
                t.z,
                if secs > 0.0 { dist / secs } else { 0.0 },
                looping as u8
            );
        }
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
