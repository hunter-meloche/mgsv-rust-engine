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

## `.mtar` motion archives — container decoded

Implemented in `crates/fox-data/src/mtar.rs` (`mgsv mtar PKG.fpk:INNER`).
Player locomotion and actions live in
`/Assets/tpp/pack/player/motion/player2_location_motion.fpk`
(`player2_cqc.mtar`, `player2_behind.mtar`, `player2_carry.mtar`, ...).

* Header (0x20 bytes): magic `0x0C012B72`, u32 clip count, u16 18, u16 56,
  u32 flags (`0x10000005`), u32 end of entry table (`0x20 + 32 * count`).
* Entries (32 bytes): u64 path hash (QAR layout; extension id = `gani`),
  u32 clip offset, u16 frame count (twice), u32 track count, u32 0,
  u32 auxiliary-block offset, u32 0.

## `.gani` clips — in progress

Observed at a clip offset (example: a 278-frame, 35-track CQC clip):

* A short header (u32 count, zero padding, u32 `0x04000000`, u32 `0x400`).
* A table of u32 track descriptors: low byte is a format code (`0x0c`,
  `0x20` seen), upper 24 bits a data offset relative to the clip; offsets
  increase monotonically. Short trailing tracks 2 bytes apart look like
  constant channels.
* The auxiliary block (pointed to by the entry) holds `StrCode32` hashes with
  flags and offset lists — most likely bone bindings and per-track channel
  descriptions.

Next: find the engine's clip decoder. `MtarFile`/`GaniBaseFile` are
registered as classes at `0x141a757a0`/`0x141a77840` (descriptor names
only); the decoder is reached from the code that consumes those
descriptors, or from the motion players that reference `mtarFile` (about
25 functions around `0x140a4c8c0`–`0x140a83970`).
