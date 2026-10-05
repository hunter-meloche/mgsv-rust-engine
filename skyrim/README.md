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
Snake mode, and Skyrim's weapon/shield attachment nodes are hidden (the
equipped weapon still gets drawn, just not shown).
**F10** cycles debug write modes (2 = full, default; 1 = local only; 0 = off).

Firing: while aiming, the left mouse button fires Snake's weapon. Snake's
controller decides when shots happen (MRS-4: 720 rpm full auto, 30-round
magazine, reload, recoil); each shot is launched as a real Skyrim projectile
on the main thread (SKSE task interface), so Skyrim handles hits, damage and
reactions. Shots fly along the crosshair ray, starting at the depth of
Snake's right hand, with MGSV's muzzle speed and drop (MRS-4 450 m/s).
The bullet is a private in-memory copy of the vanilla Hunting Bow, Iron
Arrow and arrow projectile records (no pick-up, `BulletDamage` in
`snake.ini`, default 48: 4x the original 12, since it is a rifle round); the vanilla records are untouched. Its model is
`meshes\snake\bullet.nif`, an empty model from `mgsv skyrim-export` (the
path is a Skyrim pooled string made with `BSStringPool::GetEntry`, AE id
69161, checked by looking up the arrow's own model path first), so bullets
fly unseen and leave nothing stuck in what they hit. Hits count as Archery. Damage follows MGSV's own
damage table (`DamageParameterTables.lua`): each weapon's lethal damage is
scaled so the MRS-4's 600 gives `BulletDamage`; the WU S.Pistol has none
(sleep damage 750 instead), so its darts do no damage and carry Skyrim's
`Paralyze05` poison: verified in game, one dart drops an NPC. Verified in game: a 9-round
burst at ~6 m took most of an NPC's health. Needs the Address Library for SKSE Plugins database
(`versionlib-1-7-104-0.bin`) in `Data/SKSE/Plugins`.

The gun: `mgsv skyrim-export` (run by `deploy.ps1`) converts the MRS-4 from
your MGSV install into `Data\meshes\snake\wp_west_ar_010.nif` and
`Data\textures\snake\*.dds` (FMDL to BSTriShape in Skyrim axes/units; FTEX
base colour passed through as DXT1, the two-channel normal map rebuilt with
MGSV's specular map as the mask). The plugin loads it with
`BSModelDB::Demand`, attaches it to the right hand and places it from
Snake's grip each frame while aiming or reloading; otherwise it rides on
its MGSV holster point (`CNP_ASRROOT`, at the left hip), placed relative to
Skyrim's `NPC Spine2` (snake_retarget_weapon_place).

Aim camera: in Snake mode, aiming frames Snake over his right shoulder as in
MGSV, with Snake on the left and the gun in view. The plugin sets Skyrim's
drawn-hands camera offsets (`fOverShoulderCombatPosX` / `PosZ`, read live by
the game) to `AimCameraX` / `AimCameraZ` from snake.ini (default 120 / 0;
`AimCamera=0` leaves them alone) and puts the player's own values back when
Snake mode ends. It finds the settings by name: an INI setting is
`{vtable, value, name}`, so it is the object in the exe's data whose `+0x10`
points at the name string. After Snake mode ends, Skyrim's camera keeps the
offset until the next draw or sheathe.

First person: Snake's arm chains (clavicles down to the fingers; the
first-person skeleton has finger bones, so his grip shows) drive the
first-person skeleton too, while its spine and head stay Skyrim's so the
camera is untouched. Each clavicle takes the world orientation of its
third-person counterpart (Skyrim's first-person spine is posed very
differently), so the rifle points where you look, held low on the right.
Snake's own arms replace Skyrim's (see Snake's body below). Guns and flash
attach to the first-person hand as separate copies of the meshes (`*_1p.nif`; Skyrim caches models by path). With the pistol the
shoulders drop 10 units so the two-handed grip does not cover the
crosshair. The WU S.Pistol's textures ship without `.ftex` headers
(only the two largest mips exist); the export rebuilds them from the
`.ftexs` parts (`Ftex::from_parts`: chunk table at offset 0, square size,
format from the slot). Keys **1** / **2** switch between the MRS-4 and the
pistol (one mesh per weapon, all attached to the hand); the pistol has no
holster point on Snake's body, so it is hidden when lowered. Snake's body has connection points for the rifle
(`CNP_ASRROOT`) and the sniper rifle (`CNP_SNRROOT`) but none for a
handgun, so the pistol is not shown when lowered.

Muzzle flash: `meshes\snake\muzzle_flash.nif`, built from MGSV's own flame
flipbook (`fx_mzffir01`) as additive crossed quads, attached beside the gun
and moved to the weapon's `CNP_MUZZLE_FLASH` for ~45 ms per shot. It is a
separate mesh on purpose: the plugin writes only local transforms for the
gun and flash and lets Skyrim derive world transforms (writing them itself,
as it does for bones, made the renderer draw the gun as a flat
fog-coloured silhouette on some frames).

Gunshot sound: each weapon plays the shot event MGSV itself posts for the
player, exported by `mgsv skyrim-export` to
`Data\SKSE\Plugins\snake_sounds\<weapon>_shot.wav` and played with XAudio2
(`ShotVolume` in `snake.ini`). Wwise event names are hashed (FNV-1 of the
lower-case name); the game builds the player's shot event names in code
from pieces kept as plain strings in the exe: `sfx_w_p_` + the weapon's
sound set (EquipParameters: the receiver row's last field indexes
`receiverParamSetsSound`, e.g. `ar00`, `hg01`) + `_sup` when its muzzle
option is a suppressor (`muzzleOption` flag), else `_m` (handguns leave it
out) + `_nonact` for non-lethal weapons, else `_active` (the game's
"non-active" weapons, `Player.IsNonActiveBySlot`, are the non-lethal ones:
machine guns only have `_active`, the stun SMG has `_nonact`). The export
derives the name from the tables (`WeaponStats::shot_events`) and mixes the
event from its layers (transient, body, low thump, reverb tail; each with
its delay, volume and pitch; `fox_data::wwise`, `mgsv sound-event`):
MRS-4 `sfx_w_p_ar00_m_active` (2.6 s with its tail), WU S.Pistol
`sfx_w_p_hg01_sup_nonact`. The plugin gives each sound enough voices for 14
shots a second to ring out (37 for the MRS-4). Not modelled yet: the
suppressor's durability (30 shots for the WU S.Pistol in the tables).

Exported meshes: Fox triangles wind the other way from Skyrim's, so the
export reverses them (checked against vanilla meshes, whose triangles face
along their vertex normals).

Snake's body: `mgsv skyrim-export` also writes `meshes\snake\snake_body.nif`
(body, face and bionic arm, 54 shapes) as a Skyrim SE skinned mesh in the
layout of the vanilla body meshes (vertex-less BSTriShape,
BSDismemberSkinInstance, NiSkinData, NiSkinPartition holding the vertices).
It is bound to the 20 core skeleton bones the in-game third-person skeleton
has; every Snake bone maps through the retarget table (fingers, toes and
twists fold into their parents). The retarget now also places every joint Snake
drives where Snake's is, so the skeleton takes his proportions and the
mesh needs no joint fix-ups: `mgsv skyrim-body-check` skins the export with
the retargeted skeleton over a scripted run and compares with Snake's own
skinning, 0.2-0.4 cm mean error (max ~14 cm at the hands, whose fingers the
third-person skeleton cannot move). The plugin loads it, points each skin
instance's bones at the live skeleton (the bone count comes from the skin
data, +0x58: `numMatrices` stays 0 until the shape has rendered) and
attaches it to the player's 3D root; in Snake mode it is shown and Skyrim's
body, armour and head hidden (control `body=0` keeps Skyrim's). Verified in
game standing, aiming, running, crouched and prone, and Skyrim's own body
comes back when Snake mode ends. Meshes in mesh groups MGSV flags invisible
(the face's wrinkle-map helper quads) are left out.

In first person the plugin attaches `meshes\snake\snake_arms_1p.nif` to the
first-person skeleton the same way: the same parts cut down to the vertices
weighted mostly below the clavicles, bound to the 46 first-person arm,
twist and finger bones (all in the bind pose of the retargeted third-person
skeleton, since the plugin writes the same locals to both). Skyrim's
first-person arms are hidden while it shows, so you see Snake's gloved right
hand and bionic left arm on the gun.

Movement speed: Skyrim's character controller still moves the player
(collision, slopes, stairs) but at Snake's speed. Snake's controller runs
MGSV's locomotion internally (root motion: run ~2.35 m/s, crouch-run ~1.6,
prone crawl ~0.7), and the player's `SpeedMult` is set to
100 x Snake's speed / Skyrim's speed at 100 for the current Skyrim gait,
times `MoveSpeedScale` (snake.ini, percent, default 100)
(run measured at 5.0 m/s, learned in play from steady stretches; samples
from walls, turns and staggers are rejected). Idle presets stay at 35 or
more because Skyrim will not start moving from the crawl's ~13. The
player's own `SpeedMult` is restored when Snake mode ends and before every
save, and re-read after loading. Movement and gait come from Skyrim's input
flags; while aiming, the flags are Snake's strafe stick.

Verified in game: run (Snake 2.35 / Skyrim 2.3 m/s), sprint (4.2 / 4.3),
aim strafe (2.0 / 1.9), crouch and prone. Skyrim keeps sprinting until the
player stops, even after the sprint key is released, and its sprint still
uses stamina.

How it runs: Snake's controller and the retarget run on the plugin's own
60 Hz thread, which holds still whenever the game stops updating the player
(menus, tutorial pop-ups, loading), so Snake does not walk or fire into a
paused world; the hook on `PlayerCharacter::UpdateAnimation` (called from
Skyrim's job threads) only copies the latest pose into the bone nodes and
never calls into `snake.dll` (calling Rust from the job threads froze the
game). If anything faults, Snake mode switches itself off.

Testing hooks: `Data/SKSE/Plugins/snake_control.txt` (`enabled=1`, `mode=2`,
`stance=prone`, `move=run`, `aim=1`, `fire=1`, `weapon=1`, `speed=0`, `flash=0`, `launch=0`, `draw=0` to keep hands
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

copies `SnakeSKSE.dll` and `snake.dll` into `Data\SKSE\Plugins`, writes
`snake.ini` with the paths to your MGSV install and this repo's dictionaries,
and (if `target\release\mgsv.exe` is built) exports the weapon meshes and
textures. Launch with `skse64_loader.exe`. To uninstall, remove those three
files and the `Data\meshes\snake` and `Data\textures\snake` folders.
