/* snake.h: C API for Snake's player systems (snake.dll / libsnake).
 *
 * Built from crates/snake-ffi. Everything is read from the user's own
 * MGSV: TPP install at runtime; nothing from the game is embedded.
 *
 * Conventions: Fox space (Y up, meters; a heading of 0 faces +Z and the right
 * side of a heading is -X); quaternions are x, y, z, w. Functions never
 * unwind into C: failures return NULL / 0 and set snake_last_error().
 * Players are not thread-safe; use each from one thread at a time.
 */
#ifndef SNAKE_H
#define SNAKE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SnakeRuntime SnakeRuntime;
typedef struct SnakePlayer SnakePlayer;

typedef struct { float x, y, z; } SnakeVec3;
typedef struct { float x, y, z, w; } SnakeQuat;

/* One tick of input. Booleans are 0/1. move_* is the stick relative to the
 * camera (x right, y forward, length 0..1). switch_to: weapon slot or -1. */
typedef struct {
    float move_x, move_y;
    float camera_yaw;  /* radians about +Y; 0 looks along +Z */
    float aim_pitch;   /* radians, up positive */
    uint8_t sprint, walk, crouch, prone, aim, fire, reload;
    int32_t switch_to;
} SnakeInput;

typedef struct {
    SnakeVec3 point, normal;
    int64_t id; /* host object id, or -1 */
} SnakeHit;

/* The host's world. raycast returns 1 and fills *hit when the segment
 * from -> to hits something. slide moves an upright character capsule
 * (feet position) by delta, sliding along obstacles, and writes the new feet
 * position to *out (pre-filled with feet + delta); NULL means no obstacles.
 * Arguments are pointers so any FFI can implement the callbacks. */
typedef struct {
    void *user;
    int32_t (*raycast)(void *user, const SnakeVec3 *from, const SnakeVec3 *to, SnakeHit *hit);
    void (*slide)(void *user, const SnakeVec3 *feet, const SnakeVec3 *delta, float radius, float height, SnakeVec3 *out);
} SnakeWorld;

typedef struct {
    SnakeVec3 origin, direction;
    float speed;   /* m/s, from the game's bullet table */
    float gravity; /* m/s^2 bullet drop */
} SnakeShot;

/* stance: 0 stand, 1 crouch, 2 prone. gait: 0 idle, 1 walk, 2 run, 3 dash.
 * weapon: slot in use or -1. recoil: camera pitch kick (radians) to add. */
typedef struct {
    int32_t stance, gait, aiming, weapon, ammo, reserve, reloading;
    float recoil;
} SnakeStatus;

/* Last error on this thread (empty string if none). */
const char *snake_last_error(void);

/* Load assets from a game install. dictionaries_dir is the repo's
 * dictionaries/ folder. NULL on failure. */
SnakeRuntime *snake_runtime_open(const char *game_dir, const char *dictionaries_dir);
void snake_runtime_free(SnakeRuntime *rt); /* players stay valid */

uint32_t snake_bone_count(const SnakeRuntime *rt);
const char *snake_bone_name(const SnakeRuntime *rt, uint32_t bone); /* NULL if unknown */
int32_t snake_bone_parent(const SnakeRuntime *rt, uint32_t bone);   /* -1 for the root */
uint32_t snake_weapon_count(const SnakeRuntime *rt);

SnakePlayer *snake_player_new(const SnakeRuntime *rt);
void snake_player_free(SnakePlayer *p);

/* Advance by dt seconds (fixed rate recommended, e.g. 1/60). world may be
 * NULL. Returns 1 on success. */
int32_t snake_player_update(SnakePlayer *p, const SnakeInput *input, const SnakeWorld *world, float dt);

void snake_player_transform(const SnakePlayer *p, SnakeVec3 *position, float *yaw);
void snake_player_set_transform(SnakePlayer *p, SnakeVec3 position, float yaw);

/* Bone transforms from the last update in the character's root space.
 * Writes up to capacity entries per array (either may be NULL); returns the
 * bone count. */
uint32_t snake_player_pose(const SnakePlayer *p, SnakeVec3 *positions, SnakeQuat *rotations, uint32_t capacity);

/* Shots fired since the last call (world space). Returns the number written. */
uint32_t snake_player_take_shots(SnakePlayer *p, SnakeShot *out, uint32_t capacity);

void snake_player_status(const SnakePlayer *p, SnakeStatus *out);

#ifdef __cplusplus
}
#endif

#endif /* SNAKE_H */
