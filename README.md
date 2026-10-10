# MGSV Rust Engine

A Rust reimplementation of the Fox Engine runtime behind *Metal Gear Solid V:
The Phantom Pain* (PC), built from reverse-engineering research. Like the
[Skate 3 Rust Engine](https://github.com/SK8-ENGINE/skate-3-rust-engine), it
runs on **your own copy of the game**: the engine reads the original archives
and runs the game's own Lua scripts and data at runtime. No game code, data
or assets are included in this repository.

The first part to reach a playable state is **Snake himself**: MGSV's player
movement, animation, aiming, weapons and sounds, packaged as a library. The
library runs inside *Skyrim Special Edition* through an SKSE plugin, so you
can play as Snake in Skyrim, with MGSV's own animations, guns and sounds
loaded straight from your MGSV install.

Single-player only: MGO and the online features are out of scope.

## What works so far

### Engine

- **Archives.** Every retail archive mounts and reads: QAR `.dat` with both
  decryption layers, `.fpk`/`.fpkd` packages, patch priority, and hash name
  resolution through community dictionaries.
- **Game data formats.** Every retail file parses for each of these formats:
  - `.fox2` DataSets: 75,967 files.
  - `.fmdl` models: 14,555 files, with skeletons, skinned meshes and
    materials.
  - `.mtar`/`.gani` motion: 7,572 clips.
  - `.frig` animation rigs.
  - `.fcnp` connection points.

  Also decoded:
  - `.ftex` textures.
  - Wwise sound banks (`.sbp`), including events, switches and containers.
- **Game scripts.** A Lua 5.1 host runs the game's own scripts. 1,115 native
  functions are mapped from the executable, and the engine's `vars` layout is
  recovered. `mgsv run` boots the `init` mission, goes through the pre-title UI
  and reaches mission 10010 (the prologue) in title mode.
- **Not yet built:** a renderer for the full game, the open world, AI and
  missions. See [docs/ROADMAP.md](docs/ROADMAP.md).

### Snake's player systems (`snake-core`, `snake-runtime`, `snake-ffi`)

- **Animation.** MGSV's own clips are solved on MGSV's rig, with arm and leg
  IK, root motion, finger poses, and pose-space blending between clips.
- **Movement:**
  - Stand, crouch and prone.
  - Walk, run, dash and crawl at MGSV's speeds.
  - Start, stop and turn clips.
  - Stance changes without stopping, including while aiming.
- **Prone moves:**
  - The dive.
  - Prone rolls.
  - Lying on his back: scooting, aiming and firing, and rolling back over when
    the aim swings round.
- **Weapons:**
  - The AM MRS-4 rifle and the WU S.Pistol (tranquilizer). Stats, damage and
    bullet ballistics come from the game's own tables.
  - Aim pose grids, recoil, and MGSV's reload clips.
  - Drawing and holstering: the rifle hangs on its sling, which swings, and
    the pistol sits in its thigh holster.
- **Sound.** Footsteps, crawling, suit and gear rattle and gunshots are mixed
  from MGSV's own sound banks, with MGSV's surface and action switches.
- **Physics.** The strap at Snake's back is simulated with MGSV's own physics
  parameters.
- **C API.** `snake.dll` (header `crates/snake-ffi/include/snake.h`) lets
  another engine host Snake. See [docs/BRIDGE.md](docs/BRIDGE.md).

### Snake in Skyrim (the SKSE plugin)

Press **F8** in game to turn the player into Snake:

- **His body and animation.** His body, face and bionic arm, converted from
  your MGSV install, replace the player's. MGSV's animation drives Skyrim's
  skeleton, retargeted to Skyrim's bones, and his root motion moves the
  player at MGSV's speeds. Skyrim still handles gravity, slopes and
  collision.
- **Stealth.** His stance drives Skyrim's sneaking: crouched and prone count
  as sneaking, and prone adds a Sneak bonus.
- **Camera.** It frames him the way MGSV does, close over the right shoulder
  when aiming. First person shows his own arms and the gun in hand.
- **Shooting.** Shots fire as real Skyrim projectiles at the gun's muzzle
  speed and drop, so Skyrim handles hits and damage. The tranquilizer darts
  drop NPCs with a paralysis poison.
- **On-screen and sound.** A HUD counter shows the weapon and ammo. Muzzle
  flash, gunshots and movement sounds come from MGSV.

The full details, with every value measured or derived from the game, are in
[skyrim/README.md](skyrim/README.md).

| Key | Action |
| --- | --- |
| **F8** | Toggle Snake mode |
| **W A S D** | Move (Snake turns toward the camera-relative direction) |
| **Left Alt** (hold) | Dash. Prone, with A or D: roll |
| **Ctrl** | Tap: stand / crouch (from prone: crouch). Hold: go prone, or from prone stand up |
| **Space** | Dive (while running or dashing) |
| **Right mouse** | Aim (draws the gun) |
| **Left mouse** | Fire |
| **R** | Reload |
| **1** / **2** | AM MRS-4 / WU S.Pistol |
| **F** | First / third person (Skyrim's own toggle) |

Settings live in `Data\SKSE\Plugins\snake.ini`, which `deploy.ps1` writes
with comments. They cover the motion tempo, bullet damage, volumes (gunshots,
movement sounds), the camera, unlimited magazines, the dash key and the
prone Sneak bonus.

## Setting it up yourself

### What you need

- **Windows 10 or 11.**
- ***Metal Gear Solid V: The Phantom Pain*** (Steam). The engine reads its
  archives, and the Skyrim plugin loads Snake from it at runtime.
- **For the Skyrim mod:**
  - ***Skyrim Special Edition***, runtime **1.7.104** (the version the
    plugin is built for).
  - [SKSE64](https://skse.silverlock.org/) **2.3.1** for that runtime.
  - The [Address Library for SKSE Plugins](https://www.nexusmods.com/skyrimspecialedition/mods/32444):
    its `versionlib-1-7-104-0.bin` goes in `Data\SKSE\Plugins`.
- **[Rust](https://rustup.rs/)** with the MSVC toolchain, stable 1.85 or
  newer (the code uses edition 2024).
- **Visual Studio 2022** with the *Desktop development with C++* workload.
  It provides the MSVC compiler and CMake.
- **[Git](https://git-scm.com/)** and **PowerShell 7** (`pwsh`).

### 1. Get the code

Clone the repository into your MGSV folder (Steam: right-click the game,
*Manage → Browse local files*):

```powershell
cd "<Steam>\steamapps\common\MGS_TPP"
git clone <this repo> mgsv-rust-engine
cd mgsv-rust-engine
```

The tools find the game in the parent folder. To use an install elsewhere,
set `MGSV_GAME_DIR` or pass `--game`.

### 2. Build the engine and tools

```powershell
cargo build --release
```

This builds:
- `target\release\mgsv.exe`, the command-line tool.
- `target\release\snake.dll`, Snake as a library.

To check that it reads your install:

```powershell
.\target\release\mgsv.exe info
.\target\release\mgsv.exe play
```

`mgsv play` opens a test window where you drive Snake around a test range:
WASD, Shift dash, C crouch, Z prone, right mouse aim, left mouse fire, R
reload, Esc quit. There's no Skyrim needed for this step.

### 3. Build the Skyrim plugin

The plugin builds against the SKSE64 source that ships with SKSE, found in
Skyrim's `src\` folder after you install SKSE. It also needs ianpatt's
`common` library. Both are built into the git-ignored `.tools\` folder:

```powershell
$cmake = "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
mkdir .tools\skse
tar -xzf "<Skyrim>\src\skse64-2.3.1.tar.gz" -C .tools\skse
git clone https://github.com/ianpatt/common .tools\skse\common
& $cmake -B .tools\skse\common\build -S .tools\skse\common -A x64 -DCMAKE_INSTALL_PREFIX=extern
& $cmake --build .tools\skse\common\build --config Release --target install
& $cmake -B skyrim\snake-skse\build -S skyrim\snake-skse -A x64
& $cmake --build skyrim\snake-skse\build --config Release
```

`<Skyrim>` is your *Skyrim Special Edition* folder. If your Visual Studio
edition isn't Community, adjust the CMake path. The result is
`skyrim\snake-skse\build\Release\SnakeSKSE.dll`.

### 4. Install into Skyrim

```powershell
pwsh skyrim\deploy.ps1 -Skyrim "<Skyrim>"
```

This does four things:
- Copies `SnakeSKSE.dll` and `snake.dll` into `Data\SKSE\Plugins`.
- Writes `snake.ini` there, pointing at your MGSV install and this repo's
  dictionaries.
- Runs `mgsv skyrim-export`, which converts Snake's body, arms, weapons,
  muzzle flash and gunshot sounds from **your** MGSV install into Skyrim's
  `Data\meshes\snake`, `Data\textures\snake` and
  `Data\SKSE\Plugins\snake_sounds`.
- Leaves Skyrim's own records and files untouched.

Run it again after every rebuild. It rewrites `snake.ini` with the defaults.

### 5. Play

1. Launch Skyrim with `skse64_loader.exe` and load a game.
2. Press **F8**.

The plugin's log is
`Documents\My Games\Skyrim Special Edition\SKSE\SnakeSKSE.log`. If anything
in Snake mode faults, the plugin switches itself off and logs why.

To uninstall:
- Remove `SnakeSKSE.dll`, `snake.dll`, `snake.ini` and the `snake_sounds`
  folder from `Data\SKSE\Plugins`.
- Remove the `Data\meshes\snake` and `Data\textures\snake` folders.

## Command-line tools

`target\release\mgsv.exe` works directly on your installed archives
(`mgsv --help` lists everything):

| Command | What it does |
| --- | --- |
| `mgsv info` | Mounted archives, entry counts, name-resolution coverage |
| `mgsv ls [FILTER]` / `mgsv find PATTERN` | List files / find files inside packages |
| `mgsv cat PATH` | Print one file, e.g. `/Assets/tpp/script/lib/TppMain.lua` |
| `mgsv extract [FILTER] [--unpack-fpk]` | Extract to `extracted/` (gitignored) |
| `mgsv run [--frames N] [--watch EXPR]...` | Boot and run the game's scripts from the `init` mission, following mission switches |
| `mgsv fox2` / `fmdl` / `mtar` / `frig` / `fcnp PATH` | Dump one file of that format. Each has a `-check` variant that parses every file in the game |
| `mgsv play` | Drive Snake around a test range in a window |
| `mgsv anim-view` | Local HTML viewer of Snake posed by clips |
| `mgsv skyrim-export --skyrim DIR` | Export Snake's meshes, textures and sounds for the Skyrim plugin |
| `mgsv sound-render` / `sound-event` | Render MGSV sound events to WAV |
| `mgsv stance-probe`, `draw-probe`, `hand-view`, ... | Development probes for the player systems |

On Git Bash, set `MSYS_NO_PATHCONV=1` so `/Assets/...` arguments are not
rewritten into Windows paths.

## Layout

| Path | Contents |
| --- | --- |
| `crates/fox-hash` | CityHash64 v1.0.x, `StrCode32/64`, QAR path hashing |
| `crates/fox-archive` | QAR (`.dat`) and FPK/FPKD readers, decryption, game-install VFS |
| `crates/fox-data` | Game formats: `.fox2`, `.fmdl`, `.mtar`/`.gani`, `.frig`, `.fcnp`, `.ftex`, Wwise banks |
| `crates/fox-lua` | Lua 5.1 host for the game's scripts; native modules; mission loading |
| `crates/fox-anim` | Clip sampling, rig solve with IK, skinning |
| `crates/snake-core` | Snake's controller: movement, stances, aiming, weapons, foley, strap and sling |
| `crates/snake-runtime` | Loads Snake's assets from the install; retargeting onto Skyrim's skeleton |
| `crates/snake-ffi` | `snake.dll`, the C API (`include/snake.h`) |
| `crates/skyrim-data` | Skyrim BSA/NIF/plugin readers and NIF writing for the export |
| `crates/mgsv-cli` | The `mgsv` command-line tool |
| `skyrim/` | The SKSE plugin (`snake-skse`), its deploy script and notes |
| `dictionaries/` | Name dictionaries for resolving hashes (GzsTool and FoxTool, MIT) |
| `re/` | Reverse-engineering workflow: Ghidra scripts and derived API metadata |
| `tools/` | Analysis scripts (Python, standard library only) |
| `docs/` | Format notes, architecture, the Skyrim bridge design and the roadmap |

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
[GhidraMCP](https://github.com/LaurieWired/GhidraMCP). The Skyrim plugin uses
[SKSE64](https://skse.silverlock.org/) and the
[Address Library for SKSE Plugins](https://www.nexusmods.com/skyrimspecialedition/mods/32444).

This is an unofficial project, not affiliated with or endorsed by Konami or
Bethesda. *Metal Gear Solid V* and *Fox Engine* are trademarks of Konami
Digital Entertainment; *The Elder Scrolls V: Skyrim* is a trademark of
ZeniMax Media.
