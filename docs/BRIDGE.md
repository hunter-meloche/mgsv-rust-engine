# Bridging Snake into another engine (Skyrim via SKSE)

The player systems build as a DLL (`cargo build --release -p snake-ffi` →
`target/release/snake.dll`) with a C header at
`crates/snake-ffi/include/snake.h`. A host engine links it, feeds input and
world queries each tick, and reads back the character's transform, skeleton
pose and shots. `tools/snake_ffi_demo.py` is a complete minimal host (Python
`ctypes`): it gives the DLL a wall through the callbacks, walks Snake into it,
aims and fires.

Everything is read from the user's own MGSV: TPP install at runtime
(`snake_runtime_open(game_dir, dictionaries_dir)`); nothing from the game is
embedded in the DLL.

## Per-frame flow

```
startup:   rt = snake_runtime_open(game, dicts); p = snake_player_new(rt)
           snake_player_set_transform(p, spawn, heading)
each tick (fixed 1/60 s recommended):
           fill SnakeInput from the host's controls and camera
           snake_player_update(p, &input, &world, dt)
           snake_player_transform(p, &pos, &yaw)       -> move the host actor
           snake_player_pose(p, positions, rotations)  -> drive the host skeleton
           snake_player_take_shots(p, shots, n)        -> spawn host projectiles / hit tests
           snake_player_status(p, &status)             -> HUD, camera recoil
```

`SnakeWorld` callbacks are how the host's physics enter: `raycast` (bullets,
muzzle obstruction) and `slide` (character capsule movement). Both take
pointers, so any FFI can implement them.

## Coordinates: Fox ↔ Skyrim

| | Fox (this library) | Skyrim |
|---|---|---|
| Up | +Y | +Z |
| Forward at heading 0 | +Z | +Y (north) |
| Right of forward | -X | +X |
| Units | meters | game units, about 70 per meter (1 unit ≈ 1.43 cm) |
| Heading | radians about +Y, left-positive | `angle.z` radians, clockwise (right-positive) |

Position: `sky = (-fox.x, fox.z, fox.y) * 70`; inverse `fox = (-sky.x, sky.z, sky.y) / 70`.
The axis map has determinant +1 (a proper rotation), so rotations convert by
the same basis change. Heading: `sky_angle_z = -fox_yaw`.

## Skyrim plugin plan (SKSE / CommonLibSSE-NG)

1. **Load**: on `kDataLoaded`, call `snake_runtime_open` with the MGSV install
   path from the plugin's INI; create a player.
2. **Take over movement**: disable vanilla player movement/attack controls
   while "Snake mode" is on; read the raw controller/keyboard state and the
   third-person camera yaw/pitch into `SnakeInput`.
3. **World**: implement `raycast` with the cell's Havok world
   (`bhkWorld::PickObject` / `bhkPickData`), converting units; implement
   `slide` with the player's `bhkCharacterController` (or a capsule cast),
   so Snake collides with Skyrim geometry.
4. **Apply**: each frame, `SetPosition`/`SetAngle` the player reference from
   `snake_player_transform`.
5. **Show Snake**, two options:
   * *Retarget* `snake_player_pose` onto Skyrim's skeleton
     (`NPC Pelvis` ← `SKL_000_WAIST`, `NPC Spine*` ← spine/chest,
     `NPC L/R UpperArm/Forearm/Hand` ← arm chain, `NPC L/R Thigh/Calf/Foot`
     ← leg chain, fingers ← `SKL_1xx/2xx`), writing local rotations after the
     animation graph update. Fastest path; Snake's motion on a Skyrim body.
   * *Snake's own model*: convert his `.fmdl` meshes locally (on the user's
     machine, from their install) to a NIF with his skeleton and drive it
     directly. Truer look, more work.
6. **Weapons**: for each `SnakeShot`, run the host hit test (or launch a
   projectile with the shot's speed/gravity) and apply damage; play muzzle
   effects at the shot origin.

## Status

Implemented and tested in this repo: the DLL, header, callbacks, the Python
host demo. Not started: the SKSE plugin itself (C++), retargeting tables.
