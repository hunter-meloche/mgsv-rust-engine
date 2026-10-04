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

## Next

- [ ] Engine to Lua messages (`TppMain.OnMessage`, StrCode32 sender/message ids) and native mission-load stages, to advance `Seq_Mission_Prepare`
- [ ] Remaining ~50 unmapped modules (`Fox`, `fox.KEY_*`, class objects)
- [ ] The 25 script-used `vars` declared outside `0x140996b70` (`sortiePrep*`, `mbm*`)
- [ ] Title mission (`title_fpkd`, `s00005`) after `init`
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
