# Snake in Skyrim (SKSE plugin)

`snake-skse/` is an SKSE64 plugin that loads `snake.dll` (this repo's
`crates/snake-ffi`) inside Skyrim Special Edition and runs Snake's player
systems from your own MGSV: TPP install. See `docs/BRIDGE.md` for the design.

Status: **phase B works in game** — press **F8** to toggle Snake mode:
Snake's animation drives the player's skeleton (retargeted onto Skyrim's
bones). Skyrim still moves the character, and Snake follows Skyrim's own
state: walk / run / sprint from the player's movement flags, **crouch while
sneaking** (Skyrim's sneak key), **X** toggles prone, the **right mouse
button** raises the weapon. While aiming, the player's (empty) hands are
drawn, so the actor turns with the camera and strafes like Snake's aim walk;
aim pitch follows the camera. Skyrim's own attack/block input is off in
Snake mode.
**F10** cycles debug write modes (2 = full, default; 1 = local only; 0 = off).

Firing: while aiming, the left mouse button fires Snake's weapon. Snake's
controller decides when shots happen (MRS-4: 720 rpm full auto, 30-round
magazine, reload, recoil); each shot is launched as a real Skyrim projectile
on the main thread (SKSE task interface), so Skyrim handles hits, damage and
reactions. Shots fly along the crosshair ray, starting at the depth of
Snake's right hand, with MGSV's muzzle speed and drop (MRS-4 450 m/s).
The bullet is a private in-memory copy of the vanilla Hunting Bow, Iron
Arrow and arrow projectile records (no pick-up, `BulletDamage` in
`snake.ini`, default 12); the vanilla records are untouched. It still uses
the arrow model. Needs the Address Library for SKSE Plugins database
(`versionlib-1-7-104-0.bin`) in `Data/SKSE/Plugins`.

How it runs: Snake's controller and the retarget run on the plugin's own
60 Hz thread; the hook on `PlayerCharacter::UpdateAnimation` (called from
Skyrim's job threads) only copies the latest pose into the bone nodes and
never calls into `snake.dll` (calling Rust from the job threads froze the
game). If anything faults, Snake mode switches itself off.

Testing hooks: `Data/SKSE/Plugins/snake_control.txt` (`enabled=1`, `mode=2`,
`stance=prone`, `move=run`, `aim=1`, `fire=1`, `draw=0` to keep hands
sheathed, `dump=1` to log the camera) drives the plugin without key presses.
Log: `Documents/My Games/Skyrim Special Edition/SKSE/SnakeSKSE.log`.

Bones: 25 of the 61 retargeted bones exist in the third-person skeleton.
The rest (fingers, toes, forearm twists) are only in Skyrim's first-person
skeleton; the third-person body meshes are skinned to the 24 bones that are
there, so every bone that deforms the visible body is driven.

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
