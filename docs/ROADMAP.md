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

## Next

- [ ] Map the 78 modules registered through class reflection (`Fox`, `Script`, `GrTools`, `fox.KEY_*` ...)
- [ ] The 25 script-used `vars` declared outside `0x140996b70` (`sortiePrep*`, `mbm*`)
- [ ] `TppEquip` equipment ids (`EQP_*`) and the remaining unrecovered constants
- [ ] Drive the title sequence after boot (`TppMain`, `TppSequence`, mission load)
- [ ] `.fox2` DataSet parser (entities and properties); see FoxTool
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
