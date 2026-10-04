# Roadmap

## Done

- [x] CityHash64 v1.0.x, `StrCode32/64`, QAR path hashing (`fox-hash`)
- [x] QAR reader with both decryption layers and zlib (`fox-archive`)
- [x] FPK/FPKD reader with script de-obfuscation
- [x] Game-install VFS with patch priority; `mgsv` CLI (info/ls/cat/extract/fpk)
- [x] Static scan of the native Lua API used by all 1,204 scripts (`re/api/lua_native_api.json`)
- [x] Lua 5.1 host with traced phantoms for unimplemented natives
- [x] Natives: `Script.LoadLibrary*`, `dofile`/`loadfile` via archives, `Fox` logging/hash/platform, `foxmath`, `bit` (LuaBitOp), `SplashScreen`, `TppScriptVars` declarations and typed `gvars`/`svars`
- [x] Boot runs `init.lua` and `start.lua` (coroutine-driven) up to native player vars
- [x] Ghidra headless pipeline and export scripts

## Next

- [ ] Finish Ghidra analysis and run the binding export; record recovered native constants (`TppScriptVars.*`, `TppGameObject.*`, `fox.KEY_*`, ...) in `re/api/`
- [ ] Engine-declared `vars` (player state) from the binary's declaration tables
- [ ] Boot through `start.lua` and `start2nd.lua` into the title sequence (`TppMain`, `TppSequence`)
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
