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

- [ ] `.frig` skeleton and `.fmdl` mesh parsers (Snake's body and weapons)
- [ ] `.gani`/`.mtar` animation parsers and a playback core
- [ ] Player controller: stances (stand, crouch, prone, crawl), dive, sprint, cover; recover parameters from the binary and `.fox2` player data
- [ ] Camera: over-the-shoulder, shoulder swap, aim zoom
- [ ] Weapons: equip parameters (`EquipParameters.lua`, `TppEquip` ids), recoil, ballistics, reload
- [ ] Physics/collision interface the controller runs against (pluggable, so Skyrim collision can be fed in)
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
