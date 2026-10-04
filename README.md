# MGSV Rust Engine

A Rust reimplementation of the Fox Engine runtime behind *Metal Gear Solid V:
The Phantom Pain* (PC), built from reverse-engineering research. Like the
[Skate 3 Rust Engine](https://github.com/SK8-ENGINE/skate-3-rust-engine), it
runs on **your own copy of the game**: the engine reads the original archives
and runs the game's own Lua scripts at runtime. No game code, data or assets
are included in this repository.

Status: early. The archive layer is complete and verified against the retail
data, and the game's own boot scripts run in the Rust Lua host up to the
point where native player state is needed. There is no renderer yet. See
[docs/ROADMAP.md](docs/ROADMAP.md).

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
| `mgsv boot [--all]` | Run the game's boot scripts in the Rust Lua host and report native API use |

On Git Bash, set `MSYS_NO_PATHCONV=1` so `/Assets/...` arguments are not
rewritten into Windows paths.

## Layout

| Path | Contents |
| --- | --- |
| `crates/fox-hash` | CityHash64 v1.0.x, `StrCode32/64`, QAR path hashing |
| `crates/fox-archive` | QAR (`.dat`) and FPK/FPKD readers, decryption, game-install VFS |
| `crates/fox-lua` | Lua 5.1 host for the game's scripts; Rust implementations of native modules |
| `crates/mgsv-cli` | The `mgsv` command-line tool |
| `dictionaries/` | Path dictionaries for resolving archive hashes (from GzsTool, MIT) |
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
