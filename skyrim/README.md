# Snake in Skyrim (SKSE plugin)

`snake-skse/` is an SKSE64 plugin that loads `snake.dll` (this repo's
`crates/snake-ffi`) inside Skyrim Special Edition and runs Snake's player
systems from your own MGSV: TPP install. See `docs/BRIDGE.md` for the design.

Status: **phase B works in game** — press **F8** to toggle Snake mode:
Snake's animation drives the player's skeleton (retargeted onto Skyrim's
bones). Skyrim still moves the character, and Snake follows Skyrim's own
state: walk / run / sprint from the player's movement flags, stance from
**Ctrl** as in MGSV (a tap goes up one stance: prone -> crouch, crouch <->
stand; holding it 0.35 s goes prone, or from prone stands up), the **right mouse
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

Camera: in Snake mode the camera frames Snake as MGSV does. It sits a fixed
180 units (2.6 m) behind him, with no wheel zoom, 35 to his right and 12 up.
Aiming, it pulls in 135 close over his right shoulder (side 50, height -10),
with Snake at the left of the frame. These values were matched against MGSV
screenshots. MGSV keeps its camera values in code, not data. The plugin sets
Skyrim's own [Camera] settings, which the game reads live. The distance is
fVanityModeMinDist = fVanityModeMaxDist (the zoom range). The side and
height are fOverShoulderPosX / PosZ. Aiming uses fOverShoulderCombatPosX /
PosZ and fOverShoulderCombatAddY, which Skyrim reads when the hands are drawn;
a positive AddY pulls the camera in (Skyrim's horse and dragon cameras use
-300 and -600 to pull back). Settings are found by name: an INI Setting is
`{vtable, value, name}`. The player's own values come back when Snake mode
ends. Keys in snake.ini: `CameraDistance`, `CameraX`, `CameraZ`,
`AimCameraX`, `AimCameraZ`, `AimCameraAddY`; `AimCamera=0` keeps Skyrim's
camera. The control file takes the same keys live for tuning. Its
`probe=7` lists every [Camera] setting with its value.

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
It is bound to 50 bones: the 20 core bones the in-game third-person skeleton
has, plus the 30 finger bones. Every Snake bone maps through the retarget
table; toes and twists fold into their parents. The live third-person
skeleton has no finger bones, so the export also writes each hand's 15 as a
node tree in Snake's rest pose, relative to the hand
(`meshes\snake\snake_fingers_l.nif` / `_r.nif`, root `SnakeFingers L/R`).
The plugin attaches each tree under the live hand before binding the body's
skin. The retarget output already has the finger bones, driven by Snake's,
so they take his MGSV finger poses, including his grip on each gun. The
world pass brings a tree's root up to its hand before the fingers. The
layout check skips the finger bones and runs once per skeleton.

The strap hanging at Snake's back moves under physics, as in MGSV. It is a
four-bone chain, `SKL_310_BELT_A`..`D`, which MGSV simulates rather than
animates. `snake_core::strap` steps it at 120 Hz with MGSV's own parameters
from `sna0_main0_body.sim` (a Fox `SimOnPhysics` dataset):
- link masses 0.2/0.2/0.2/0.8 kg, damped;
- the top joint limited to 35 degrees from hanging slightly back, the
  others to 90 degrees from their parent link;
- collision capsules on the waist, thighs and shins;
- the ground, so it lies on the legs when Snake is prone;
- inertia coefficient 0.6: 60% of Snake's movement reaches it as inertia.
The retarget adds the four bones as `Snake Strap A`..`D` under `NPC Pelvis`
(`EXTRA_BONES`). The export ships them as `meshes\snake\snake_strap.nif`,
and the plugin attaches the tree under the live pelvis like the fingers.

Foley: Snake's footsteps, hands on the ground, suit and gear rattle,
weapon rattle and the chest landing on a dive play from MGSV's own sound
bank, chosen the way MGSV's `SoundTppPlayer.lua` and `motion/setup.lua`
choose them:
- footsteps and hands: `FOOT_GROUND_L`..`ARM_LEAVE_R` with
  `player_type_switch` = `nom` and `foot_step_switch` = surface + action
  (`SOILA_Rn`, `ROCKA_cr`, ...); actions `Wk`, `WkC`, `Rn`, `RnC`, `Dh`
  (dash), `cr` (crawl), `cp` (getting down prone);
- suit rattle `sfx_p_rtl_suit_nom_wk` (`rattle_suit_switch` = action) with
  every step and stance change;
- weapon rattle `RATTLE_WEAPON` (`rattle_weapon_switch` = `ar00_rn`, ...)
  for rifles; MGSV's handguns have no rattle;
- the dive: hands, then the chest landing `Ref_P_Chest_Ground`;
- raising the gun to aim and switching weapons rattle the suit and gun.
Clips with motion events (stance changes, the dive, the crawl) play them at
their frames (`left_foot_ground`, `rattle_suit`, `slide`, ...); the
locomotion loops have none in the archive (MGSV's motion graph adds them),
so steps there are found in the pose, a foot coming down to its lowest
height. `snake_runtime::foley` resolves the Wwise switch and random
containers (`fox_data::wwise::Hirc::render_layers`) and mixes the layers
at 48 kHz; the plugin plays them on a pool of 24 XAudio2 voices. The ground
is stone (`ROCKA`) in interiors and soil (`SOILA`) outdoors; Skyrim's
per-ground material and wet ground are not read yet. Volume: `FoleyVolume`
(percent) in snake.ini, `FootstepVolume` (percent of that, default 100) for the feet and hands on the ground, and `RattleVolume` (default 10) for the suit and weapon rattle, which played far louder than the rest of the game. `mgsv bank-events` and `mgsv sound-render` list and
render the bank's events to WAV with chosen switches.

The retarget also places every joint Snake drives where Snake's is, so the
skeleton takes his proportions and the mesh needs no joint fix-ups.
`mgsv skyrim-body-check` skins the export with the retargeted skeleton over
a scripted run and compares it with Snake's own skinning: mean 0.0 cm, max
0.2 cm idle and running, 1.4 cm aiming, 4 cm crawling. The hands are exact;
before the finger bones they were off by about 14 cm. The plugin loads it, points each skin
instance's bones at the live skeleton (the bone count comes from the skin
data, +0x58: `numMatrices` stays 0 until the shape has rendered) and
attaches it to the player's 3D root; in Snake mode it is shown and Skyrim's
body, armour and head hidden (control `body=0` keeps Skyrim's). Verified in
game standing, aiming, running, crouched and prone, and Skyrim's own body
comes back when Snake mode ends. Meshes in mesh groups MGSV flags invisible
(the face's wrinkle-map helper quads) are left out.

Materials: MGSV shades physically based. Its specular map holds
reflectance (R), roughness (G) and metalness (B), and each material's
`MatParamIndex_0` picks a row of the global preset table
`/Assets/fox/effect/gr_pic/material_params.fmtt`. Column 0 is the base
reflectance: fatigues 0.012, skin 0.035, the bionic arm 0.16. Skyrim has
one Phong lobe per shape, so the export bakes a per-pixel specular mask
(the normal map's alpha). It is reflectance (bare steel's where metallic)
times the peak of a normalised Blinn-Phong lobe of that pixel's roughness,
doubled for the reflections Skyrim lacks. The shape's glossiness is the
exponent of the mask-weighted mean roughness. Before this, a flat mask at
gloss 30 made the rough fabric look like plastic. Materials without a
normal map (the hair) get a flat one carrying their mask; otherwise
Skyrim's default normal map gave the hair a full shine. Base textures that
cut out with alpha (hair cards, eyelashes, the Diamond Dogs patch) are
alpha-tested at 128 (`NiAlphaProperty` 0x12EC) and drawn double-sided.

In first person the plugin attaches `meshes\snake\snake_arms_1p.nif` to the
first-person skeleton the same way: the same parts cut down to the vertices
weighted mostly below the clavicles, bound to the 46 first-person arm,
twist and finger bones (all in the bind pose of the retargeted third-person
skeleton, since the plugin writes the same locals to both). Skyrim's
first-person arms are hidden while it shows, so you see Snake's gloved right
hand and bionic left arm on the gun.

Movement: Snake moves the player himself (direct drive). Skyrim's movement
code sets the player's Havok character controller velocity once per frame
through `bhkCharacterController::SetLinearVelocityImpl` (vtable slot 7 of the
`bhkCharProxyController` at player +0xF8 (process) +0x08 +0x250, all checked
by RTTI name; the slot was found by counting per-frame calls). In Snake mode
that call's horizontal velocity is replaced by Snake's own root motion,
turned into Skyrim's world axes by the actor's heading, so starts, stops and
strafes follow MGSV's own animations at MGSV's speeds (run 2.35 m/s, crouch
1.6, crawl ~0.7), while gravity, slopes and collision stay Skyrim's; only the
player's controller is changed (the vtable is shared). Measured in game
(`trace=1` in the control file logs both speeds every 0.1 s): Skyrim's speed
follows Snake's through the start (0 -> 0.6 -> 1.0 -> 1.8 -> 2.35 m/s), the
stop and every stance, within about 0.05 m/s. If the controller is not found,
the earlier SpeedMult matching takes over (Skyrim's run measured at 5.0 m/s,
SpeedMult = 100 x Snake's speed / Skyrim's);
`drive=0` in the control file forces it. The player's own SpeedMult is left
alone while driving. Dash: Snake dashes (MGSV's sprint, `s_dh_lp`, 4.2 m/s x tempo) only while
the sprint key is held: `SprintKey` in snake.ini, a virtual-key code,
default 164 = Left Alt, Skyrim's sprint key. Releasing it goes back to the
run. There is no stamina cost, as in MGSV: Skyrim's `SprintHandler` (found in
PlayerControls by RTTI name, before the ready-weapon handler) is off in
Snake mode. It drained stamina and kept sprinting after the key was
released. Dashing while moving crouched rises straight into the dash: the
controller switches to standing and enters the dash loop at the frame that
best matches the crouched stride. Snake stays standing afterwards. Prone he
keeps crawling.

Dive: Space while running or dashing (standing or crouched, not aiming)
plays MGSV's dive to prone, `snapdiv_s_rn_f_int`: 6.1 m in 4.4 s, about
3 s at tempo 1.5. That variant ends exactly in the prone idle
`snapnon_c_idl_l`; the plain `snapdiv_s_rn_f` ends mirrored. Snake is prone
afterwards: sneaking, the crawl Sneak bonus, a low camera. While Snake moves,
Skyrim's jump is off, so Space only dives. Standing still upright, Space
still jumps. The control file's `dive=1` dives once.

Tempo: `MoveSpeedScale` in snake.ini (percent, default 150) sets the
tempo of all of Snake's motion: his controller advances by 1.5 x the real
time step, so every animation plays half again as fast, and through root
motion he moves 1.5x as fast (run 3.5 m/s). Turns, stance changes, raising
the gun and reloads speed up with it. The fire rate stays the weapon's
rounds per minute. 100 is MGSV's own tempo. Facing: Snake decides where he faces, as in MGSV: without aiming he turns
toward camera heading + stick direction (his own turn rate and turn clips)
and runs that way; aiming, he turns to the camera and the stick strafes.
Skyrim's actor heading is left alone, because its third-person camera keeps
its yaw relative to it (writing the heading spun the camera); instead
`NPC Root [Root]` is turned by Snake's facing minus the actor's heading
(Skyrim's animation graph otherwise turns that node toward the steering
direction itself and snaps it back on stopping), and his root motion is
turned into the world by his own facing. Snake's yaw and Skyrim headings
are tied by one fixed offset taken when Snake mode starts. While aiming, Snake's stick is Skyrim's own movement input
(`PlayerControls` +0x24: x right, y forward, keys or gamepad; the actor's
movement flags cannot tell back from right while aiming), so strafes go
the way the keys say (checked in game for W/A/S/D). Skyrim's jump is off
while Snake is crouched or prone (`JumpHandler`, two `PlayerControls` slots
before the attack handler, checked by RTTI name like the ready-weapon
handler).

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

Stance and stealth: in Snake mode Ctrl (read by the plugin; Skyrim's
`SneakHandler`, found beside the attack handler and checked by RTTI, is off)
sets Snake's stance, and Skyrim's sneaking follows it: crouched and prone
count as sneaking (the HUD eye shows), standing does not. Skyrim does the
toggle itself. Writing only the sneak flag (actor state bit 9) left the HUD
eye out of step, and so did the graph events `sneakStart`/`sneakStop`.
Whenever Skyrim's sneak state differs from Snake's, a "Sneak" `ButtonEvent`
is replayed as a fresh press through the handler's own ProcessButton on the
main thread. That is vtable slot 6 on this runtime: slots 2-5 are the base
class's shared no-ops, and CommonLib's slot 4 is SE's. The event is the
first real one Ctrl sends, or until then one built from any key's button
event, with the pooled "Sneak" control id and Ctrl's key code. Skyrim refuses toggles
while the hands are drawn or sheathed (just after aiming) and for a moment
after. No toggle is tried during that, and a refused one is retried every
0.3 s. Only after 20 refusals in a row is the flag written directly. Ctrl taps and holds count
from the stance Snake is heading for, so quick taps chain (prone -> crouch
-> stand) while a transition still plays.

Stance changes on the move do not stop Snake, as in MGSV. Standing <->
crouching while walking or running switches at once and blends into the
new stance's loop from the matching point of the stride (MGSV has no clips
for it). Down to prone and up from the crawl play MGSV's moving transitions
(`snapnon_c_qwk2c` / `qrn2c` from a walk / run, `c2qrn` / `c2srn` up into a
crouched / standing run; the foot variant closest to the current pose),
which carry their own forward motion and continue straight into the new
stance's loop. While aiming (walking or standing still), standing <->
crouching keeps the aim: the stance switches at once and the aiming upper
body blends from one stance's aim grid to the other's over 0.3 s. Standing
still and not aiming, the stance-change clips play as before. `mgsv
stance-probe` runs these through the controller (speed, clips, largest
per-tick joint turn per stretch).

Prone, Snake gets a hidden +20 Sneak (`CrawlSneakBonus` in snake.ini;
MGSV keeps its stance camouflage values in code rather than data, only the
sight ranges and camouflage thresholds are in `Soldier2ParameterTables`, so
this is the fallback), an actor value modifier (Sneak = 15) added on the
main thread, removed on getting up, when Snake mode ends and before every
save. The cameras follow Snake's head: Skyrim's third-person camera tracks
`Camera3rd [Cam3]` and the first-person one `Camera1st [Cam1]`; both are
lowered by Snake's head height below his standing height (smoothed), so the
view drops crouched and to the ground prone, in both views, and the
first-person arms move down with it.

Ammo and reloads: **R** reloads (Skyrim's ready-weapon action, also on R,
is switched off in Snake mode like its attacks: the ReadyWeaponHandler is
found in PlayerControls six slots before the attack/block handler). Snake
reloads by himself when the trigger is pulled on an empty magazine.
Magazines are unlimited by default (`UnlimitedMagazines` in snake.ini; 0
uses the weapon's reserve from MGSV's tables). The HUD shows the weapon and
its magazine at the bottom right, on Skyrim's own HUD movie (an SKSE
Scaleform text field in the HUD font): `AM MRS-4    27 / --`, amber at a
quarter left and red when empty; Skyrim's arrow counter is hidden meanwhile.

The reloads are MGSV's own clips, picked like the game does: a quick
magazine change when rounds are left, the full reload (with the charging
handle or slide) when empty, and crouched versions where they exist. The
clip is applied as its change from its first frame (every reload clip starts
and ends in its stance's aim pose), so it starts and ends exactly on the
current pose, at any aim angle and also prone, where MGSV has no clip of its
own; when not aiming, Snake first raises the gun. The clips' motion events
mark the magazine leaving the gun and going back in: the rounds go in at
that frame, and the magazine (MGSV keeps it a separate model on the gun's
`CNP_AMMO`; exported as `<weapon>_mag.nif`, and the gun's built-in copy,
mesh group `MESH_AMO`, is left out of the gun) rides in Snake's left hand in
between. Measured with `mgsv reload-probe`: no joint turns faster per tick
than in the clips themselves (MRS-4 quick change 2.9 s, rounds in at 1.8 s;
full reload 6 s, rounds in at 3.3 s). Mesh groups the game hides (the MRS-4's
rail light) are no longer exported.

Drawing and holstering: the guns ride on Snake's body and come out with
MGSV's own motions. Aiming (or reloading) from holstered plays the draw
clip on the upper body (applied as its change from its standing aim frame
onto the current stance's aim, so crouched it starts and ends kneeling;
not started during a stance change, whose stance is only settled at its
end) (`snapasr_s_fre0_st` for the rifle,
`snaphg01_s_fre0_st` for the pistol), after a short reach for the gun since
the clips start with the hand on it; letting go plays the holster clip
(`..._fre0_ed`), which ends in the idle arms. The rifle hangs on its sling
at the left hip (the body's `CNP_ASRROOT` on `SKL_300_ASRROOT`). The sling
bone swings a little with Snake's movement (`snake_core::sling`): the
rifle keeps its mounted angle (muzzle down and back, the stock up in front
of the chest, as in MGSV) on a spring, free to swing 15 degrees side to
side but only 8 fore and aft, out of the left thigh. MGSV's own sling
simulation (`sna0_main0_asr.sim`: a body 13.6 cm from the joint in a
35-degree cone) left to hang under gravity stood the rifle up from the hip
into the shoulder whenever Snake leaned to run or crouch. The pistol's
right-thigh holster is closed: MGSV models the holstered gun into Snake's
body (its grip shows at the back of the holster; mesh 16, in a mesh group
under the holster's), so the pistol is hidden in there and that grip is
shown only while the pistol is in it (`snake_weapon_holster_shape`; the
plugin hides the shape while the gun is out). The gun's place inside the
holster is on its helper bone `SKL_534_RHOL_HLP` (no connection point):
turned as the draw clip holds it where the hand closes on it (barrel
down), and moved into the holster mesh (the body mesh skinned to that
bone), its top 12 cm centred on the holster's top. The
clips mark the hand-over with two unnamed motion events (`f55cc7aa` draw,
`634c2eca` holster; found by where the gun is). The gun changes hands where
it comes closest to its holster up to that event: the rifle's holster event
comes after the hand has already turned the rifle 57 degrees away from the
sling, which swung it. Its transform then blends between hand and holster
over 0.12 s. A weapon switch puts the gun in the hand away first, then
draws the other one if Snake is still aiming. Prone (MGSV has standing
clips only) the gun changes hands at once. Firing waits until the draw is
done. `mgsv draw-probe` prints the hand-to-holster distance per clip frame
and runs draws, holsters and switches through the controller; `mgsv play
--draw-test --view-yaw 140 --shot f.png --shot-every 3` renders them.

Testing hooks: `Data/SKSE/Plugins/snake_control.txt` (`enabled=1`, `mode=2`,
`stance=prone` (once: X and sneak decide again when it is reached), `move=run`, `aim=1`, `fire=1`, `reload=1`, `weapon=1`, `tempo=0.25` (Snake's tempo, for slow-motion checks), `speed=0`, `flash=0`, `launch=0`, `draw=0` to keep hands
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
