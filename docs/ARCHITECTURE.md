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
| `.xdata` (117 MB, RWX) | Protector layer; contains the PE entry point. Left out of the local analysis database (`re/ghidra/PrepareMgsv.java` drops the block before auto-analysis) and never analysed, modified or bypassed; the executable on disk is not changed. |
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

### Libraries and `requires`

`Script.LoadLibrary` publishes a script's returned table under its file stem.
If that table has a `requires` array, the engine loads every listed library
once, as a dependency (`0x140104d80`). `Tpp.lua` uses this to pull in the 52
core libraries, including `TppMain` and `TppSequence`. Paths without a leading
`/` are rooted at `/`.

### Missions

A mission package (`.fpkd`) contains a `TppSimpleMissionData` entity. Its
`script` property names the shim (`/Assets/tpp/script/mission/mission_main.lua`)
and its `subScripts` map names `sequence`, `enemy`, `demo`, `radio` and
`sound` to scripts in the same package. The shim's hooks forward to `TppMain`.
The engine calls each hook with a table holding only the sub-scripts
(`TppMain.OnAllocate` walks it with `pairs`). `OnAllocate`/`OnInitialize` run
as coroutines, because they yield while waiting for saves and loads.
`OnUpdate` runs once per frame.

### Headless defaults

Unimplemented natives are phantoms. Unimplemented predicates follow one rule:
every asynchronous operation has already finished. Completion checks
(`IsEnd*`, `IsFinish*`, `IsReady*`, `IsLoaded*`, `Is*Active`, ...) return true,
and other predicates (`IsSaving*`, `IsPlaying*`, `Has*`, ...) return false.
Both kinds of wait loop therefore terminate. Natives whose answer matters are
implemented for real: `TppEquip.IsExistFile` checks the archive index, and
`TppScriptVars.IsSavingOrLoading`/`GetLastResult` report no save in flight.

### Frame loop and engine systems (`game.rs`)

`FoxLua::advance_frame` is one engine frame: clock, event timers, script
block loads and native UI flows. It also runs between resumes of coroutine
hooks, so a script that waits inside `OnInitialize` sees its loads finish.
`Game::frame` runs `OnUpdate`, delivers svars notifications and messages,
advances the frame, then follows mission switch requests.

* **svars notifications.** Writes that change an `svars` value queue
  `(name, index)`. The engine passes them to the mission's
  `OnChangeSVars(name, index)` hook. `TppSequence` switches sequences there
  (`svars.seq_sequence`); `TppSequence.Update` does nothing until
  `mvars.seq_currentSequence` is set by that path.
* **Messages.** Delivered as `OnMessage(scripts, class, message, arg0..arg3)`
  with `StrCode32` ids. Scripts key handlers by class and message names, and
  optionally by sender (`arg0`). A handler can return
  `Mission.ON_MESSAGE_RESULT_RESEND` to get the message again next frame,
  with `Mission.GetCurrentMessageResendCount` counting up. (The value of that
  constant is host-defined until the `Mission` module is mapped.)
* **Event timers** (`GkEventTimerManager.Start/StartRaw/Stop/StopAll/
  IsTimerActive/SetPaused`) run on the simulated clock. On expiry they post
  `Timer`/`Finish` with the timer's `StrCode32` as sender. For example,
  `Seq_Mission_Prepare` leaves its final state through `Timer_WaitStartingGame`.
* **Mission switching.** Scripts register package path functions
  (`Mission.SetMissionPackagePathFunc`), set `vars.missionCode`, and call
  `Mission.LoadMission{...}`; the request carries only options. The host runs
  `OnTerminate`, resolves the packages through the registered function, and
  loads the new mission. All Lua files in the mission's packages run as
  libraries before the mission's main script and sub-scripts (e.g.
  `title_sequence.lua`, `cypr_mission_block.lua`). Sub-scripts load in an
  order where every script follows the sub-scripts whose globals it references.
* **Script blocks.** Declared by `ScriptBlockData` entities (`name`,
  `enabled`, `sizeInBytes`, `prerequisites`) in the mission's DataSets.
  `ScriptBlock.Load(id, packages)` moves a block to PROCESSING. The next
  frame runs the packages' scripts and the block becomes INACTIVE, or ACTIVE
  if `Activate` came during the load (`TppScriptBlock.Load` activates right
  after requesting).
* **Fades.** `FadeFunction.CallFadeIn/CallFadeOut(duration, a0, a1)`
  (`0x1405c7c80`) runs one fade at a time on the simulated clock. When it
  ends it posts `UI`/`EndFadeIn` or `EndFadeOut` with `a0, a1`; scripts pass
  the fade's name hash as `a0` and match it as the sender.
* **Splash screens.** `SplashScreen.Create/Show/SetStateCallback/Delete/
  GetSplashScreenWithName`. When a shown splash finishes it is removed and
  its callback gets `(handle, STATE_DELETE = 5)`. The publisher logos chain
  through these callbacks.
* **Save slots.** `TppScriptVars.Set/GetVarValueInSlot`, `SaveVarsToSlot`,
  `LoadVarsFromSlot` and `CopySlot` work on in-memory numbered slots. Group
  bits select stores (vars 1, gvars 4, svars 8 assumed) and category masks
  select variables. Nothing is written to files yet.
* **Mission reload.** `TppMission.Load` to the current mission calls
  `Mission.RequestToReload()` rather than `LoadMission`; the host handles
  both the same way.
* **Build identity.** `TppGameSequence.IsMaster()` is true (retail data) and
  `TppSystemUtility.GetCurrentGameMode()` is `"TPP"` (single-player scope).
* **Input stand-ins.** With no renderer or controller, `mgsv run --post`
  injects the UI messages a player's button presses would produce (for
  example `UI`/`PressStart` and `UI`/`TitleMenu` from `GameStart`).
* **Pre-title UI.** `TppUiCommand.StartPreTitleSequence` (state machine at
  `0x14085c8c0`) shows the `PreTitle` layout and then calls the script
  function `TppVarInit.StartTitle()`. That function chooses the title mission
  from the save state and requests it. Headless, the logos take no time.

### Current progress

`mgsv run --frames 600`:

* `init.lua`/`start.lua` (31 frames), then mission 1 (`init`). `init` reloads
  itself once (`LoadMission{force=true}`), as on retail. `Seq_Mission_Prepare`
  then runs through its states to `Seq_Demo_Init` and `Seq_Demo_StartTitle`.
* The pre-title flow calls `TppVarInit.StartTitle()`, which selects mission
  10010 (new game: the prologue with the title screen on top). It loads from
  five packages, and its four Cyprus script blocks stream their demo and
  small-mission packages.
* Mission 10010 finishes `OnInitialize` and enters
  `Seq_Demo_StartHasTitleMission`, the title-menu state. Here the publisher
  logo splashes play through their state callbacks.
* With `--post 200:UI:PressStart --post 300:UI:TitleMenu:GameStart` the title
  flow goes through `Seq_Game_PushStart` and `Seq_Game_TitleMenu` to
  `Seq_Game_ChunkInstalled`. The `GameStart` fade-out ends with
  `EndFadeOut` from `GameStart`, the new-game handler clears title mode, and
  mission 10010 reloads (`RequestToReload`).
* After that reload the mission starts in title mode again
  (`Seq_Demo_StartHasTitleMission`) instead of the opening. The open
  question is how retail picks the post-title start: what the new-game path
  leaves in the checkpoint slot or gvars, and how the title sequence's
  `OnEnter` reads it.

## Recovered from the binary

All of this is committed under `re/api/`; it holds identifiers and numbers only.

* `lua_bindings.json`: 1,115 native functions (1,068 of the 1,281 callable
  members scripts use) and 4,043 constants across 161 modules, registered by
  230 functions plus about 200 callbacks they hand out.
  Members are registered with a helper taking `(L, name, value_or_function)`.
  Enums are often registered in a loop over a pointer table of names, with the
  loop counter as the value. `tools/lua_bindings.py` replays each registration
  function with a register tracker. Three registration shapes are handled:
  direct calls (`name`, value or function), loops over a pointer table of
  names (the value is the loop counter), and `{u32 value; char* name}` arrays
  built on the stack (e.g. the 2,187 `TppEquip.EQP_*` ids), and `luaL_Reg`
  arrays (`{name; function}`) built on the stack (most of `TppUiCommand`).
  Code pointers that Ghidra did not define as functions are accepted when
  they point into `.xtext`. Class-style
  modules (`GrTools`, `Script`) register inside a callback handed to a static
  class descriptor. Short names may be undefined in Ghidra or pooled into the
  tail of a longer string; the exporter reads those raw. Members that scripts
  index further (`TppCommand.Weather.X`) are namespaces, never constants.
  Examples: `TppScriptVars.TYPE_*` = BOOL 0, INT32 1, UINT32 2,
  FLOAT 3, INT8 4, UINT8 5, INT16 6, UINT16 7; the `CATEGORY_*` values are
  single-bit flags; `TppGameObject.PHASE_*` = SNEAK 0, CAUTION 1, EVASION 2,
  ALERT 3.
* `vars_layout.json`: the engine-declared `vars` store (333 variables,
  `0x140996b70`). Each variable is a 24-byte descriptor:
  `{storage*, name*, StrCode32(name), flags<<24 | type<<16 | array_size}`.
  Tests check every stored hash against our `StrCode32` and check that no two
  variables overlap in storage.
* `.fox2`: every one of the 75,967 DataSets parses (2.42M entities). Class
  and property names are resolved from FoxTool's dictionary plus identifiers
  harvested from the user's own `mgsvtpp.exe` at runtime.
* About 50 modules (`Fox`, `fox` key codes, several class objects) still have
  no recovered registration.

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
