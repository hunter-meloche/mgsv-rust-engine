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
    }
}

fn boot(vfs: Vfs, all: bool) -> Result<()> {
    let host =
        fox_lua::FoxLua::new(std::sync::Arc::new(vfs)).map_err(|e| anyhow::anyhow!("{e}"))?;
    // start.lua runs start2nd.lua itself.
    for script in ["/init.lua", "/Tpp/start.lua"] {
        match host.run_coroutine(script, 100_000) {
            Ok(frames) => println!("ok    {script} ({frames} frames)"),
            Err(e) => {
                println!("FAIL  {script}\n{e}");
                break;
            }
        }
    }
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
    Ok(())
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
