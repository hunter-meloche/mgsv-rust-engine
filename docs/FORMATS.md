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
Many bone-name hashes are not in our dictionaries yet.

## `.frig` rigs

Humans ship only `/Assets/tpp/rig/frig/human_finger.frig`; animals have full
rigs (`tpp_dog.frig`, `tpp_horse.frig`, ...). The human body skeleton comes
from the `.fmdl` bones. FoxLib's reader (MIT) only decodes the header
(`u32 hash, u32 type-string offsets, u32 group count, u32 track count, u32
file size, u32 rig data end/start`); the rest is undocumented.

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

Player archives have 18 units, and none of them resolve as bone names
against about 229,000 candidates (exe identifiers, model string tables,
script strings). They are **rig units** (root, spine, and two-bone IK
chains for arms and legs), which the rig maps onto skeleton bones. Playing
them on Snake's skeleton therefore needs the `.frig` rig: its unit
definitions and the IK solve.
