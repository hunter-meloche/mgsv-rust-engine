# Snake in Skyrim (SKSE plugin)

`snake-skse/` is an SKSE64 plugin that loads `snake.dll` (this repo's
`crates/snake-ffi`) inside Skyrim Special Edition and runs Snake's player
systems from your own MGSV: TPP install. See `docs/BRIDGE.md` for the design.

Status: **phase B works in game** — press **F8** to toggle Snake mode:
Snake's animation drives the player's skeleton (retargeted onto Skyrim's
bones). Skyrim still moves the character, and Snake follows Skyrim's own
state: walk / run / sprint from the player's movement flags, **crouch while
sneaking** (Skyrim's sneak key), **X** toggles prone, the **right mouse
button** raises the weapon (aim pitch from the player's look pitch).
**F10** cycles debug write modes (2 = full, default; 1 = local only; 0 = off).

How it runs: Snake's controller and the retarget run on the plugin's own
60 Hz thread; the hook on `PlayerCharacter::UpdateAnimation` (called from
Skyrim's job threads) only copies the latest pose into the bone nodes and
never calls into `snake.dll` (calling Rust from the job threads froze the
game). If anything faults, Snake mode switches itself off.

Testing hooks: `Data/SKSE/Plugins/snake_control.txt` (`enabled=1`, `mode=2`,
`stance=prone`, `move=run`, `aim=1`) drives the plugin without key presses.
Log: `Documents/My Games/Skyrim Special Edition/SKSE/SnakeSKSE.log`.

Known gaps: fingers, toes and forearm twist bones are not reachable through
the live skeleton's child arrays (Skyrim keeps them elsewhere at runtime), so
those 36 bones stay on Skyrim's animation for now.

## Build (Windows, Visual Studio 2022 with C++)

The plugin compiles against the SKSE64 source that ships with SKSE (in
Skyrim's `src/` folder) and ianpatt's `common` library, built locally into
the git-ignored `.tools/` folder:

```powershell
$cmake = "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
mkdir .tools\skse; tar -xzf "<Skyrim>\src\skse64-2.3.1.tar.gz" -C .tools\skse
git clone https://github.com/ianpatt/common .tools\skse\common
& $cmake -B .tools\skse\common\build -S .tools\skse\common -A x64 -DCMAKE_INSTALL_PREFIX=extern
& $cmake --build .tools\skse\common\build --config Release --target install
cargo build --release -p snake-ffi
& $cmake -B skyrim\snake-skse\build -S skyrim\snake-skse -A x64
& $cmake --build skyrim\snake-skse\build --config Release
```

Built for runtime 1.7.104 with SKSE 2.3.1 (the plugin declares that version;
rebuild against the matching SKSE source for other runtimes).

## Install

```powershell
pwsh skyrim\deploy.ps1 -Skyrim "<Skyrim Special Edition folder>"
```

copies `SnakeSKSE.dll` and `snake.dll` into `Data\SKSE\Plugins` and writes
`snake.ini` with the paths to your MGSV install and this repo's dictionaries.
Launch with `skse64_loader.exe`. Remove those three files to uninstall.
