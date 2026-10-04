# Roadmap

## Done

- [x] CityHash64 v1.0.x, `StrCode32/64`, QAR path hashing (`fox-hash`)
- [x] QAR reader with both decryption layers and zlib (`fox-archive`)
- [x] FPK/FPKD reader with script de-obfuscation
- [x] Game-install VFS with patch priority; `mgsv` CLI (info/ls/cat/extract/fpk)
- [x] Static scan of the native Lua API used by all 1,204 scripts (`re/api/lua_native_api.json`)
- [x] Lua 5.1 host with traced phantoms for unimplemented natives
- [x] Natives: `Script.LoadLibrary*`, `dofile`/`loadfile` via archives, `Fox` logging/hash/platform, `foxmath`, `bit` (LuaBitOp), `SplashScreen`, `TppScriptVars` declarations and typed `gvars`/`svars`
- [x] Ghidra headless pipeline and export scripts (PowerShell)
- [x] Native binding recovery: 471 functions and 1,301 enum constants (`re/api/lua_bindings.json`)
- [x] Engine `vars` layout recovered (333 variables, hash- and overlap-validated); typed `vars` store
- [x] Boot runs `init.lua` and `start.lua` to completion
- [x] Binding recovery for callbacks, enum loops, stack arrays and short/pooled names: 585 functions, 4,043 constants
- [x] `.fox2` DataSet parser (`fox-data`): all 75,967 files parse
- [x] `requires` dependency loading (52 core libraries, `TppMain`, `TppSequence`)
- [x] Mission loading from `TppSimpleMissionData`; `init` mission runs `OnAllocate`/`OnInitialize` into `Seq_Mission_Prepare`
- [x] svars change notifications (`OnChangeSVars`), engine messages (`OnMessage`, resend), event timers
- [x] Game loop with mission switching (`Mission.LoadMission`, package path functions), loose package scripts
- [x] Script blocks from `ScriptBlockData` (load, activate, states)
- [x] Pre-title UI flow calling `TppVarInit.StartTitle`; boot reaches mission 10010's title mode
- [x] luaL_Reg stack arrays in binding recovery: 1,115 native functions mapped

## Priority: player systems ("Snake core")

Decided 2026-10-04. Build the systems that make Snake feel like MGSV first,
as a self-contained part of the engine that can later run as a library
bridged into Skyrim through an SKSE plugin (the SkyCraft model). The full
game (world, AI, missions) continues afterwards.

- [x] Engine function names imported from IHHook (165, verified against the exe)
- [x] `.fmdl` parser: all 14,555 models parse and validate (skeleton, meshes, materials, buffers)
- [x] `.mtar` motion archive container (clip table); `mgsv find` across packages
- [x] `.gani` clip decoding: 7,572 clips / 10.1M keyframes, exact frame accounting (`mgsv mtar-check`)
- [x] `.frig` rig: all 9 rigs parse; `human_finger.frig` maps the 18 player track units to bones (`mgsv frig`)
- [x] Playback core (`crates/fox-anim`): sample clips, rig solve with arm/leg two-bone IK, root motion. All 2,816 player clips pose with 0 unreachable IK targets (`mgsv anim-check`); local stick-figure viewer (`mgsv anim-view`)
- [ ] Forearm/upper-arm twist distribution (helper bones `SKL_5xx_*_HLP` follow their parents for now); Hermite interpolation flag
- [x] Skinned mesh (`fox_anim::SkinnedModel`): body, face (`plfova_sna0_face0`) and bionic arm (`plfova_sna0_arm0`) parts bound to the body skeleton by name; shaded in `mgsv anim-view`
- [ ] `.frdv` (likely helper-bone drivers) and `.fcnp` (connection points for weapons/items) next to each body model
- [x] Bone names: 153/153 of Snake's bones, 18/18 track units, all player clip names
- [x] Player controller v1 (`crates/snake-core`): stand/crouch/prone, walk/run/dash/crawl from the real clips with root motion (walk 0.59, run 2.28, dash 4.13, crouch run 1.52, crawl 0.66 m/s), stance-change clips, crossfades blended in rig space (`mgsv snake-sim`)
- [x] `mgsv play`: native test window (software rasterizer), WASD + mouse, third-person camera
- [x] Controller v2: start clips with 0-180 degree turns (`<gait>_st_<l|r><angle>_<foot>`), stop clips chosen by matching foot positions, idle foot variants
- [ ] Controller v3: in-place turns (`tn_*`), dive, cover, ledges/vaults; tuning against the exe's player parameters
- [x] Camera: over-the-shoulder follow, aim zoom (`mgsv play`); [ ] shoulder swap
- [x] Weapons v1: AM MRS-4 with stats read from `EquipParameters.lua` (720 rpm full auto, 30+210, 5.56 mm at 450 m/s, gravity 18), aim pose grid layered on the upper body, strafing, firing, bullets with drop, reload clip; `.fcnp` attachment (hand / back)
- [ ] Weapons v2: decode recoil/sway (wobbling) sets, other weapon classes (handgun `snaphg01`, sniper, shotgun), prone aiming (`snapasr_c_*`, `pose_35`), damage
- [x] Physics/collision interface: `snake_core::Collision` (raycasts for bullets, capsule sliding for movement), reference `BoxWorld`; `mgsv play` test range with cover, walls and target boards
- [ ] Recoil/sway: decode `receiverParamSetsWobbling` (7 fields, undocumented; community notes cover `receiverParamSetsBase` = fire rate, aim-assist distance, draw speed, ?, ?, iron sight x2, reload speed)
- [ ] Library API and a headless test harness; later the SKSE bridge

## Next (after the player systems)

- [x] Fades, splash screens with state callbacks, in-memory save slots, mission reload, input stand-ins (`--post`)
- [x] Title menu flow: press start and new game reach the prologue reload
- [ ] New game after reload: start the prologue's opening rather than title mode (save slot / gvars handover)
- [ ] Location loading (`Mission.LoadLocation`, location package path function, stage block groups)
- [ ] Remaining unmapped modules (`Fox`, `fox.KEY_*`, `Mission` constants, class objects)
- [ ] The 25 script-used `vars` declared outside `0x140996b70` (`sortiePrep*`, `mbm*`)
- [ ] Per-location chunk mounting from `foxfs.dat`

## Later

- [ ] Window, input and frame loop (Bevy, as in the Skate 3 project)
- [ ] `.ftex`/`.ftexs` textures → GPU (BCn), `.fmdl` meshes, `.frig` skeletons
- [ ] Title screen: UI graph (`.uig`/`.uilb`), fonts (`.ffnt`), localisation (`.lng2`)
- [ ] Terrain (`.tre2`), level blocks, streaming
- [ ] GameObject system, player controller, animation (`.gani`, `.mtar`)
- [ ] Physics (`.ph`, `.phsd`), navigation (`.nav2`), AI
- [ ] Audio (Wwise `.sbp`/`.bnk`/`.wem`)
- [ ] Save data compatibility (`TppScriptVars` slots)
