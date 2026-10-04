# File formats

Notes on Fox Engine (MGSV: TPP) formats as implemented here. "Verified" means
checked against every file of that type in a retail install.

## `.fmdl` models — verified (14,555 files, 51,220 meshes, 0 failures)

Implemented in `crates/fox-data/src/fmdl.rs`; layout follows the community
documentation in FMDL Studio (BobDoleOwndU, MIT).

* Header: `FMDL`, version f32 (TPP: 2.03/2.04), u64 section-info offset, two
  u64 block-flag masks, u32 section 0/1 block counts, u32 section 0
  offset/length, u32 section 1 offset/length.
* Section info: section 0 entries `(u16 type, u16 count, u32 offset)`, then
  section 1 entries `(u32 type, u32 offset, u32 length)`.
* Section 0 tables (type: entry size): bones 0:48, mesh groups 1:8, mesh group
  entries 2:32, mesh infos 3:48, material instances 4:16, bone groups 5:68,
  textures 6:4, material parameters 7:4, materials 8:4, mesh format infos 9:8,
  mesh formats 10:8, vertex formats 11:4, string infos 12:8, bounding boxes
  13:32, buffer offsets 14:16, LOD info 16:16, face infos 17:8, type 18:8,
  type 20:128, path hashes 21:8, string hashes 22:8.
* Section 1: 0 material vectors (float4), 2 vertex/index buffer, 3 strings.
* Mesh data: buffer offset 0 holds float3 positions; buffer offset 1 holds the
  interleaved attribute stream (stride from the mesh's second mesh format);
  buffer offset 2 holds u16 triangle indices (`first_index * 2`).
* Vertex attribute ids: 0 position, 1 bone weights (u8x4), 2 normal (half4),
  3 colour (u8x4), 7 bone indices (u8x4, into the mesh's bone group),
  8–11 UV0–3 (half2), 12/13 extra weights/indices, 14 tangent (half4).
* TPP names bones/meshes by `StrCode64` (table 22) rather than strings.

The player's body is `/Assets/tpp/chara/sna/Scenes/sna0_main0_def.fmdl` in
`/Assets/tpp/pack/player/parts/plparts_normal.fpk` (153 bones, 28 meshes).
All 153 bone names resolve (see `.frig` below).

## `.frig` rigs — verified (all 9 rigs in the game)

Implemented in `crates/fox-data/src/frig.rs` (`mgsv frig`, `mgsv frig-check`).
Format knowledge from mctrollin's GPL-3.0 "Fox Engine MTAR Tools" (FRIG
template); independent implementation.

* Header (32 bytes): name `StrCode64`, version `102`, rig unit count, segment
  count, file size, bone list offset, mask definition offset.
* Rig definition: one absolute offset per unit. Unit: `u32 type, i16 track
  count, i16 bone count, i16 parent bone, i16 parent unit, u32 pad`, then
  type-specific `i16` bone-list and segment indices. IK units (two-bone,
  arm, three-bone) first carry 16 unknown bytes and a chain-plane normal
  (float4).
* Mask definition: `u32 unit count, u32 layer count`, layer offsets relative
  to the mask definition; each layer is `StrCode32, char[12] name, f32 weight
  per unit` (Lower, Upper, Head, LArm, RArm, Mirror, Carry...).
* Bone list: `i32 count`, `(u32 rig index, StrCode32 name)` pairs.

`/Assets/tpp/rig/frig/human_finger.frig` is **the** human rig: 18 units and
56 segments, exactly the layout of every player/soldier `.mtar`. Track unit
`n` is rig unit `n` (track names are `RIG_<bone>`). Per-unit meaning (segment
types from the MTAR layout, semantics from the add-on's mapping):

| unit | type | segments | drives |
|---|---|---|---|
| RIG_ROOT | Root | quat diff, vector diff | root motion (per-frame deltas) |
| RIG_SKL_000_WAIST | Transform | quat, vector3 | waist rotation + position, root space |
| SPINE, CHEST, NECK, HEAD | Orientation | quat | rotation in root (character) space |
| LSHLD / RSHLD | Arm | quat, vector3, quat | shoulder rotation; hand IK target position; IK pole (quat, its X axis) for SHLD → UARM → FARM → HAND |
| LHAND / RHAND | Orientation | quat | hand rotation, character space |
| LTHIGH / RTHIGH | TwoBone | vector3, quat | foot IK target; IK pole for THIGH → LEG → FOOT |
| LFOOT / RFOOT | Orientation | quat | foot rotation, character space |
| LTOE / RTOE | LocalOrientation | quat | local rotation |
| LF10 / RF10 | MultiLocalOrientation | 16 × quat | local rotations of 16 consecutive bone-list bones (fingers, palm) |

Spaces, verified on every player clip (`mgsv anim-check`, crate
`fox-anim`):

* Fox character space: Y up, forward +Z, the character's left +X. Rest
  rotations are identity (FMDL bones carry translations only), so
  orientation tracks are the bones' model-space rotations directly.
* The root unit's "diff" segments are the root's rotation/position relative
  to the root at the clip's **last** frame (they reach identity/zero there),
  not per-frame deltas.
* IK target positions are in that same end-anchored space (a planted foot's
  target stays constant while the root moves); all other tracks (waist,
  orientations, poles) are relative to the root. Waist position is
  `(0, height, 0)`.
* The pole quaternion's **+X axis** is the elbow/knee bend direction. Over all
  2,816 clips this gives mean wrist/ankle swing of 22°/18–21°; every other
  axis gives about 60–90°. The rig's chain-plane normal `n` is the rest bend
  plane: rest bend direction = `rest_dir × n` (elbows back, knees forward).

Animals use their own rigs (`tpp_dog`, `tpp_horse`, ...); the Sahelanthropus
rig has 72 units.

Names: `dictionaries/mtar_tools/` (from the same add-on) plus `mgsv
bone-names`, which also brute-forces `SKL_<NNN>_<part>` names; every bone of
Snake's model (153/153) and every player track unit (18/18) resolves.

## `.mtar` motion archives and `.gani` clips — verified

Implemented in `crates/fox-data/src/mtar.rs` (`mgsv mtar`, `mgsv mtar-check`).
Format knowledge from mctrollin's GPL-3.0 "Fox Engine MTAR Tools" Blender
add-on (MTAR/GANI binary templates); independent implementation.

Result on a retail install: 390 archives, 7,572 clips, 10.1M keyframes
decoded with exact frame accounting. Every player body-motion archive
decodes. Not handled: 139 old-format (non-"mini") archives (weapon parts,
props, facial sets including the player's) and a few animal/facial "layers"
archives whose frame deltas do not sum to the clip length.

* Header (32 bytes): version `201403250`, clip count, unit count, segment
  count, shader node/unit counts, motion-point unit count, flags (`0x1000`
  mini format, `0x4000` has skeleton list), common-info offset.
* Clip table (32 bytes): path hash (QAR layout, extension `gani`), track data
  offset, sizes/offsets stored divided by 16, motion-event offset.
* Common info: linked nodes `(StrCode32 name, size, next offset relative to
  the node, pad)`. `LayoutTrack` (`0x4fbdaaef`) holds a TrackHeader (unit and
  segment counts, frame count/rate, unit offsets) and TrackUnits (name hash,
  segment count, segment descriptors whose low nibble is the segment type).
  Player archives also carry a `MotionPoints` node (`0x3b9a7784`).
* Clip: frame count, named float parameters, one flag byte per unit (loop 1,
  Hermite 2, static 4), 4-byte segment headers (component bit size, 24-bit
  data offset relative to the header), 16 bytes padding, keyframe data.
* Segment types: 0 quaternion, 1 float, 2–4 vector2–4, 5 quaternion diff,
  6 vector diff.
* Keyframes: initial value, then (unless static) `(u8 frame delta, value)`
  until the deltas reach the frame count. Quaternions are bit-packed,
  little-endian bit order: angle, two axis components (12/13/15 bits; the
  third axis component is `1 - x - y`, renormalised), three sign bits.
  Vectors and floats are byte-aligned 32-bit floats or 16-bit "anim halfs"
  (custom exponent bias: `((v & 0x7c00) + 0x1dc00) << 13` for the exponent).

### What the player tracks drive

Player archives have 18 units: the **rig units** of `human_finger.frig` (see
above), not bones. Clip names resolve through
`dictionaries/mtar_tools/mtar_dictionary.txt` (e.g.
`snapcqc_s_thw_f.gani`).

## `.fcnp` connection points — verified (820 files, 5,585 points, 0 failures)

Implemented in `crates/fox-data/src/fcnp.rs` (`mgsv fcnp`, `mgsv fcnp-check`);
layout worked out from retail files.

* Header (32 bytes): `u32 0`, `u32` first node offset, `u32` file size,
  `StrCode32` model name, `u32` name offset (relative to the hash field).
* Nodes (0x60 bytes, linked list): `StrCode32` name, `u32` name offset
  (relative to the node), `u32 0`, `u32 0x30`, `u32 0x30`, 2 x `u32 0`,
  `i32` previous offset, `i32` next offset (0 ends), `u32` parameter block
  offset (relative to the node), padding; then position, rotation quaternion
  and scale (float4 each) at +0x30/+0x40/+0x50.
* Parameter block: `u32 count`, then `(StrCode32 key, u32 offset, StrCode32
  value, u32 offset)`; each string offset is relative to its hash. `Parent`
  names the bone.

Snake's body (`sna0_main0_def.fcnp`): `CNP_RIGHT_HAND` / `CNP_LEFT_HAND`
(0.08 m along the hand bones), `CNP_ASRROOT` on `SKL_300_ASRROOT` (rifle on the
back), feet, eye, mouth, knife. Assembled rifle (`ar00_main0_aw6.fcnp`, frame:
barrel +Z, root at the grip): `CNP_MUZZLE_FLASH` (0.677 m), `CNP_LEFT_HAND`
(fore grip), `CNP_EJECT`, sights, light, laser.

**Holding the gun.** The gun's frame relative to `CNP_RIGHT_HAND` is not the
identity. Calibrating it on the centre pose of the rifle aim grid (frame 12 of
`snapasr_s_fre_pose_25`, where the rifle points straight ahead) makes every
grid pose land on exact authored angles (yaw +90/+45/0/-45/-90, pitch
+75/+37.5/0/-37.5/-75) with the off hand on the gun's fore grip (5.5 cm).

## Aim pose grids

`snap<weapon>_<s|q|c>_fre_pose_<n>` clips hold one aim pose per frame,
row-major from the top-left: rows are pitch (up first), columns yaw (left
first). Rifle and handgun (`snapasr_*`, `snaphg01_*`): stand/crouch 5 x 5 at 45 /
37.5 degree steps (`pose_25`), prone 9 x 4 covering yaw +180..-180 and pitch
+75..-30 (`pose_35`); `snaprdy_*_pose_9` is 3 x 3. `snapcore::WeaponRig::calibrate`
measures each grid from its poses, so layouts need no hard-coding. The game layers them onto the upper body (the rig's `Upper`
mask) over the `snaprdy_*_fre0_*` strafe loops, whose angle suffix is the
movement direction relative to the facing (positive = left).

## `EquipParameters.lua` (weapon stats)

`/Assets/tpp/level_asset/weapon/ParameterTables/parts/EquipParameters.lua` is
a data-only call `TppEquip.ReloadEquipParameterTables2{...}` (read with
`fox_data::lua_table`). Links used, with 0-based set indices:

* `gunBasic`: weapon, receiver, barrel, magazine, stock, muzzle, ... (AM MRS-4
  `WP_West_ar_010` = `RC_30101`, `BA_30115`, `AM_30102`, `SK_30113`).
* `receiver`: id, attack id, base set, wobbling set, system set, sound set.
* `receiverParamSetsBase[i][0]`: rounds per minute (handguns 300, snipers 60,
  SMGs 900-1020, rifles 720-810).
* `receiverParamSetsSystem[i]`: equip type, reticle, trigger
  (`TRIGGER_FULLAUTO`...).
* `magazine`: id, equip id, rounds, reserve rounds, bullet.
* `bullet`: id, speed, speed, gravity (9.8 for grenades; 18 for 5.56 mm,
  4.5 for sniper rounds), ...

Not yet decoded: the remaining base-set fields and the wobbling (recoil/sway)
set.
