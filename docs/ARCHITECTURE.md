# Architecture and findings

Notes on how the retail game is put together and how this project mirrors it.
Everything here was verified against the Steam build
`Tpp_steam_mst_en_day3900Mgo_patch_0707_1632`.

## Strategy

MGSV's gameplay logic is mostly Lua. About 1,200 scripts ship in the archives:
mission flow, enemy setup, Mother Base management, UI flow, radio. They run on
top of a large native API: about 200 modules and 7,400 referenced members, of
which about 1,300 are callable. The plan has three parts:

1. Read the original data directly (`fox-archive`).
2. Run the original scripts unchanged in an embedded Lua 5.1 (`fox-lua`).
3. Reimplement the native API in Rust, one module at a time. The order is
   driven by what the scripts actually call (`mgsv boot` trace,
   `re/api/lua_native_api.json`), and the behaviour comes from analysing
   the engine binary.

Later crates add rendering, animation, physics and audio as the scripts need
them, in the same way the Skate 3 project grew around its data formats.

## Executable

| Section | Notes |
| --- | --- |
| `.xtext` (34 MB, RX) | Engine code, plaintext. Analysed with Ghidra. |
| `.edata` / `.data2` | Read-only data / data (strings, vtables, tables) |
| `.xdata` (117 MB, RWX) | Protector layer; contains the PE entry point. Removed before analysis (`re/ghidra/PrepareMgsv.java`) and never touched. |
| `.ecode` | Exception directory (`.pdata`), ~141k function entries |

* Image base `0x140000000`, no relocations.
* Embeds **Lua 5.1.5** plus **LuaBitOp** (`bit.*`). Only the math types
  (`Vector3`, `Quat`, `Matrix4`, `Color`...) are in static `luaL_Reg` tables;
  the `Tpp*`/Fox modules are registered by code at startup. That is why
  `ExportLuaBindings.java` walks the registration functions instead.

## Archives (`fox-archive`)

`master/` holds QAR archives (`SQAR` magic):

* Header and section table are XOR-masked. Each section is
  `block << 40 | hash`, with 1 KiB or 4 KiB blocks (flag `0x800`).
* Entry ids are `extension_id << 51 | path_hash`. `path_hash` is
  CityHash64 **v1.0.x** with seeds `(0x9ae16a3b2f90404f, last 8 bytes
  reversed)`, masked to 50 bits. Bit 50 is set for paths outside `/Assets/`.
  CityHash 1.1 gives different results, so the 1.0 algorithm is reimplemented
  in `fox-hash`.
* Entry data goes through layer 1 (position-keyed XOR from a fixed table),
  then optionally layer 2 (LCG keystream, headers `0xA0F8EFE6` /
  `0xE3F8EFE6`), then zlib.
* `/foxfs.dat` (in `chunk0.dat`) defines the mount layout: per-location chunks
  (`cypr`, `base`, `afgh`, `mtbs`, `mafr`, `mgo`) with paired texture
  archives, plus routing of some file types to `data1.dat` by extension id.
  Five hash-named "safiles" (`master/e2f*.dat`) are ASF/WMV movies, not
  archives.
* Patches live in `master/1/<NAME>/0/*.dat` (listed in `/foxpatch.dat`) and
  override the base data.

The VFS currently merges everything into one priority-ordered view (patches
→ `master/0` → `data1` → chunks → textures). Per-location chunk mounting will
be added when level loading needs it.

Resolution with the bundled dictionary: 129,672 unique entries, 79% named.
Unresolved entries are almost all texture streams (`.ftexs`).

FPK/FPKD packages (`foxfpk win` / `foxfpkdwin`) contain the level data. Lua
scripts inside them are XOR-obfuscated with `!StrCode64(file name)` when
their first byte is `0x1B`/`0x1C`.

## Script host (`fox-lua`)

* `mlua` with the `lua51` feature builds the same interpreter version.
  LuaBitOp is reimplemented in Rust with identical 32-bit semantics.
* `Script.LoadLibrary(path)` runs a script from the archives and publishes its
  returned table as a global named after the file (`TppMain.lua` → `TppMain`).
* Boot order, as the engine does it: `/init.lua`, then `/Tpp/start.lua` run as
  a **coroutine** and resumed once per frame. `start.lua` itself runs
  `/Tpp/Scripts/System/start2nd.lua` in a nested coroutine.
* Native modules without a Rust implementation are *phantoms*: they can be
  indexed and called without erroring, and every use is recorded. When a
  phantom is used as a number (an unrecovered native constant) it reads as 0
  and is recorded separately.
* `gvars`/`svars` are typed stores (`TppScriptVars.Declare*Vars`). Values are
  coerced to the declared width, and arrays are fixed-size and 0-based.
  Out-of-range writes are dropped, because retail scripts write one past
  the end without faulting.

### Current boot progress

`mgsv boot` runs `init.lua` and `start.lua` to completion (31 simulated
frames). By then 40 libraries are loaded, the splash wait has passed, and the
title-screen variables are initialised. About 320 distinct native APIs are
still phantoms. Only three unrecovered constants are read as numbers:
`TppEquip.EQP_None` and two `TppRevenge` `MAX` values.

## Recovered from the binary

All of this is committed under `re/api/`; it holds identifiers and numbers only.

* `lua_bindings.json`: the native modules registered by 230 functions.
  Members are registered with a helper taking `(L, name, value_or_function)`.
  Enums are often registered in a loop over a pointer table of names, with the
  loop counter as the value. `tools/lua_bindings.py` replays each registration
  function with a register tracker, which yields 471 functions and 1,301
  constants. Examples: `TppScriptVars.TYPE_*` = BOOL 0, INT32 1, UINT32 2,
  FLOAT 3, INT8 4, UINT8 5, INT16 6, UINT16 7; the `CATEGORY_*` values are
  single-bit flags; `TppGameObject.PHASE_*` = SNEAK 0, CAUTION 1, EVASION 2,
  ALERT 3.
* `vars_layout.json`: the engine-declared `vars` store (333 variables,
  `0x140996b70`). Each variable is a 24-byte descriptor:
  `{storage*, name*, StrCode32(name), flags<<24 | type<<16 | array_size}`.
  Tests check every stored hash against our `StrCode32` and check that no two
  variables overlap in storage.
* 78 modules (`Fox`, `Script`, `GrTools`, ...) register through a different
  path (class reflection); they have not been mapped yet.

## Reverse-engineering workflow (`re/`)

1. `re/Run-GhidraAnalysis.ps1`: one-time headless import and auto-analysis
   (about 15 minutes). The project is stored in `re/projects/` (gitignored).
   Run the `.ps1` scripts with
   `powershell -ExecutionPolicy Bypass -File re\<script>.ps1`.
2. `re/Run-GhidraExports.ps1`: runs `ExportFunctions` and `ExportLuaBindings`.
   `tools/lua_bindings.py pair` then rebuilds `Module.Member → address` and
   constant values into `re/api/lua_bindings.json`. `tools/vars_layout.py`
   rebuilds `re/api/vars_layout.json` from an `ExportLuaBindings` run over the
   `vars` member names.
3. Put addresses in `re/out/decompile_targets.txt` and run
   `re/Run-GhidraExports.ps1 DecompileFunctions.java` to get local C
   references in `re/out/decomp/`.
4. For interactive work, install the GhidraMCP extension
   (`.tools/GhidraMCP/`) in the Ghidra GUI. That lets an assistant query the
   open database directly.
