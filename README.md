# MGSV Rust Engine

A Rust reimplementation of the Fox Engine runtime behind *Metal Gear Solid V:
The Phantom Pain* (PC), built from reverse-engineering research. Like the
[Skate 3 Rust Engine](https://github.com/SK8-ENGINE/skate-3-rust-engine), it
runs on **your own copy of the game**: the engine reads the original archives
and runs the game's own Lua scripts at runtime. No game code, data or assets
are included in this repository.

Status: early. The archive and `.fox2` layers parse every file in the retail
data. `mgsv run` boots the game's own scripts in the Rust Lua host, runs the
`init` mission's boot flow, follows its hand-off through the pre-title UI to
mission 10010 (the prologue with the title screen over it), streams that
mission's script blocks, and reaches the title-mode sequence. There is no
renderer yet; single-player only (MGO and online features are out of scope).
See [docs/ROADMAP.md](docs/ROADMAP.md).

## Install

Clone into your MGSV: TPP folder (Steam: *Manage → Browse local files*):

```
cd "<Steam>/steamapps/common/MGS_TPP"
git clone <this repo>
```

The repository ends up in `MGS_TPP/mgsv-rust-engine/` and finds the game in
its parent folder. Set `MGSV_GAME_DIR` to use an install elsewhere.

Build with Rust (MSVC toolchain on Windows):

```
cargo build --release
```

## Tools

`target/release/mgsv` works directly on your installed archives:

| Command | What it does |
| --- | --- |
| `mgsv info` | Mounted archives, entry counts, name-resolution coverage |
| `mgsv ls [FILTER] [--named]` | List files (substring filter on the path) |
| `mgsv cat PATH` | Print one file, e.g. `/Assets/tpp/script/lib/TppMain.lua` |
| `mgsv extract [FILTER] [--unpack-fpk]` | Extract to `extracted/` (gitignored) |
| `mgsv fpk PATH` | List an `.fpk`/`.fpkd` package |
| `mgsv boot [--all]` | Run the boot scripts in the Rust Lua host and report native API use |
| `mgsv run [--frames N] [--watch EXPR]... [--post F:CLASS:MSG[:SENDER]]... [--messages]` | Boot and run the game loop from the `init` mission, following mission switches. Prints events, sequence changes and watched Lua expressions; `--post` injects UI messages as stand-ins for player input |
| `mgsv fox2 PATH` | Dump a `.fox2` DataSet (`PKG.fpkd:/inner/path.fox2` for files inside packages) |
| `mgsv fox2-check` | Parse every `.fox2` in every package and report failures |
| `mgsv fmdl PATH` / `mgsv fmdl-check` | Summarise one `.fmdl` model / parse and validate every model |
| `mgsv mtar PATH` / `mgsv mtar-check` | Decode the clips of one `.mtar` motion archive / every clip in the game |
| `mgsv frig PATH` / `mgsv frig-check` | Dump a `.frig` animation rig (units, bones, masks) / check every rig |
| `mgsv bone-names` | Harvest bone-name candidates and report coverage of the player skeleton |
| `mgsv find PATTERN` | Find files inside packages, printed as `PACKAGE:INNER` |

On Git Bash, set `MSYS_NO_PATHCONV=1` so `/Assets/...` arguments are not
rewritten into Windows paths.

## Layout

| Path | Contents |
| --- | --- |
| `crates/fox-hash` | CityHash64 v1.0.x, `StrCode32/64`, QAR path hashing |
| `crates/fox-archive` | QAR (`.dat`) and FPK/FPKD readers, decryption, game-install VFS |
| `crates/fox-data` | `.fox2` DataSets (entities, typed properties) and hash name tables |
| `crates/fox-lua` | Lua 5.1 host for the game's scripts; Rust implementations of native modules; mission loading |
| `crates/mgsv-cli` | The `mgsv` command-line tool |
| `dictionaries/` | Name dictionaries for resolving hashes (GzsTool and FoxTool, MIT) |
| `re/` | Reverse-engineering workflow: Ghidra scripts and derived API metadata |
| `tools/` | Analysis scripts (Python, standard library only) |
| `docs/` | Format notes, architecture and roadmap |

## Rules for contributors

* Never commit anything taken from the game: no archives, extracted files,
  textures, models, audio, scripts, or decompiled code. `extracted/`,
  `re/out/` and `re/projects/` are gitignored for this reason.
* Decompiler output is a local reference only. Engine code in `crates/` is
  written as an original Rust implementation of the observed behaviour.
* Derived metadata (identifier names, file-format layouts, constant values,
  API signatures) may be committed under `re/api/` and `docs/`.
* The retail executable carries a protector layer. This project does not
  analyse, modify or bypass it; analysis is limited to the engine's own code.

## Reverse engineering

Windows PowerShell scripts drive a headless Ghidra install under `.tools/`
(gitignored); see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md):

```
powershell -ExecutionPolicy Bypass -File re\Run-GhidraAnalysis.ps1
powershell -ExecutionPolicy Bypass -File re\Run-GhidraExports.ps1
```

## License

Copyright (c) 2026 the MGSV Rust Engine contributors. This project's original
code is licensed under the [GNU General Public License version 3 only](LICENSE)
(`GPL-3.0-only`). You may use, modify and distribute it, including
commercially. If you distribute a modified version or a binary, you must make
the corresponding source available under GPLv3 and keep the required notices.

Third-party material keeps its own license, including the dictionaries in
`dictionaries/` (MIT; GzsTool and FoxTool). This license grants no rights to Konami's
game code, data, assets or trademarks.

## Credits

Built on years of community research into the Fox Engine, notably
[GzsTool](https://github.com/Atvaark/GzsTool) and
[FoxTool](https://github.com/Atvaark/FoxTool) (Atvaark),
[FileMonolith](https://github.com/Morbidslinky/FileMonolith) (Morbidslinky),
[RdfTool](https://github.com/kapuragu/RdfTool) (kapuragu), and the
dictionary work of the MGSV modding community. Binary analysis uses
[Ghidra](https://github.com/NationalSecurityAgency/ghidra) and
[GhidraMCP](https://github.com/LaurieWired/GhidraMCP).

This is an unofficial project, not affiliated with or endorsed by Konami.
*Metal Gear Solid V* and *Fox Engine* are trademarks of Konami Digital
Entertainment.
