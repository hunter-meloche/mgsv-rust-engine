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

`mgsv boot` runs `init.lua` to completion. In `start.lua` it loads 40
libraries, passes the splash-screen wait, declares the script variables, and
stops in `TppVarInit.SetInitPlayerWeapons`. That function writes to `vars`,
the engine-declared player state. The layout and the native constants that
`vars` needs (`TppScriptVars.PLAYER_AMMO_STOCK_TYPE_COUNT`, ...) are the
next things to recover from the binary.

## Reverse-engineering workflow (`re/`)

1. `re/run-ghidra-analysis.sh`: one-time headless import and auto-analysis
   (takes hours). The project is stored in `re/projects/` (gitignored).
2. `re/run-ghidra-exports.sh`: runs `ExportFunctions` and `ExportLuaBindings`.
   `tools/lua_bindings.py pair` then rebuilds `Module.Member → address` and
   constant values into `re/api/lua_bindings.json`.
3. Put addresses in `re/out/decompile_targets.txt` and run
   `re/run-ghidra-exports.sh DecompileFunctions.java` to get local C
   references in `re/out/decomp/`.
4. For interactive work, install the GhidraMCP extension
   (`.tools/GhidraMCP/`) in the Ghidra GUI. That lets an assistant query the
   open database directly.
