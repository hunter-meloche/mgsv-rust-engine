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
    uint8_t dive;      /* pressed: dive forward to prone (running or dashing) */
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
    float lethal_damage; /* MGSV damage per hit (MRS-4: 600) */
    float sleep_damage;  /* tranquilizer damage (WU S.Pistol: 750, no lethal) */
} SnakeShot;

/* stance: 0 stand, 1 crouch, 2 prone. gait: 0 idle, 1 walk, 2 run, 3 dash.
 * weapon: slot in use or -1. recoil: camera pitch kick (radians) to add.
 * magazine: capacity of the weapon in use; reload_progress 0..1 while
 * reloading; unlimited: reloads do not use up the reserve. */
typedef struct {
    int32_t stance, gait, aiming, weapon, ammo, reserve, reloading;
    float recoil;
    int32_t magazine;
    float reload_progress;
    int32_t unlimited;
    float head_height; /* Snake's head above his feet (m) */
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
/* Skyrim mesh of weapon i (relative to Data\meshes, written by
 * `mgsv skyrim-export`), or NULL. */
const char *snake_weapon_mesh(const SnakeRuntime *rt, uint32_t i);
/* Display name of weapon i (e.g. "AM MRS-4"), or NULL. */
const char *snake_weapon_name(const SnakeRuntime *rt, uint32_t i);
/* Name of the k-th shape of Snake's body mesh that shows weapon i in its
 * closed holster (NULL past the last); show them only while the weapon is in
 * there (snake_retarget_weapon_place returns 0). */
const char *snake_weapon_holster_shape(const SnakeRuntime *rt, uint32_t i, uint32_t k);
/* Skyrim mesh path of weapon i's magazine (frame: the gun's CNP_AMMO), or
 * NULL if it has none. */
const char *snake_weapon_magazine_mesh(const SnakeRuntime *rt, uint32_t i);
/* Muzzle of weapon i in its exported mesh's frame (Skyrim axes and units;
 * the barrel points along +Y). Returns 0 if there is no such weapon. */
int32_t snake_weapon_muzzle(const SnakeRuntime *rt, uint32_t i, SnakeVec3 *out);

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
/* Name of the clip Snake's controller plays (diagnostics); returns its length. */
uint32_t snake_player_clip(const SnakePlayer *p, char *buf, uint32_t len);
/* Unlimited magazines: reloads never use up the reserve. */
void snake_player_set_unlimited_magazines(SnakePlayer *p, int32_t on);
/* One rendered foley sound: 48 kHz stereo 16-bit interleaved samples. */
typedef struct {
    const int16_t *samples;
    uint32_t frames; /* sample pairs */
    int32_t kind;    /* 0 foot ground, 1 foot leave, 2 hand ground, 3 hand leave,
                        4 suit rattle, 5 weapon rattle, 6 chest landing */
} SnakeSound;
/* Snake's foley since the last call, rendered from MGSV's sound bank for
 * ground `surface` (MGSV footstep symbol: SOILA, ROCKA, WOODA, IRONA, GRASA,
 * GRAVA, SANDA, ...) and `wet` ground. Up to `max` sounds; samples valid until
 * the next call. Returns the count. */
uint32_t snake_player_foley(const SnakeRuntime *rt, SnakePlayer *p, const char *surface, int32_t wet,
                            SnakeSound *out, uint32_t max);
/* Tempo of all of Snake's motion (1 = MGSV's; 1.5 = half again as fast,
 * reloads included; the fire rate is unchanged). */
void snake_player_set_tempo(SnakePlayer *p, float tempo);

/* Retargeting onto Skyrim's character skeleton (read from the Skyrim
 * install's BSAs). Output bones are in parent-first order. */
typedef struct SnakeRetarget SnakeRetarget;
SnakeRetarget *snake_retarget_new(const SnakeRuntime *rt, const char *skyrim_dir);
void snake_retarget_free(SnakeRetarget *r);
uint32_t snake_retarget_count(const SnakeRetarget *r);
const char *snake_retarget_bone(const SnakeRetarget *r, uint32_t i); /* e.g. "NPC L UpperArm [LUar]" */
/* Local transforms relative to each bone's Skyrim parent (Skyrim axes and
 * units). has_translation[i] is 1 where translations[i] should be applied
 * (the COM: pelvis height). Returns the bone count. */
uint32_t snake_retarget_apply(const SnakeRetarget *r, const SnakePlayer *p, SnakeQuat *rotations,
                              SnakeVec3 *translations, uint8_t *has_translation, uint32_t capacity);

/* The weapon in Snake's hands: returns its index (-1 if none) and its
 * transform relative to Skyrim's "NPC R Hand [RHnd]" (Skyrim axes and units)
 * for the exported mesh. */
/* Where weapon `weapon`'s exported mesh goes this frame: 1 = in Snake's
 * hands (transform relative to "NPC R Hand [RHnd]"), 2 = holstered
 * (relative to "NPC Spine2 [Spn2]"), 0 = not shown. Skyrim axes/units. */
int32_t snake_retarget_weapon_place(const SnakeRetarget *r, const SnakePlayer *p, uint32_t weapon,
                                    SnakeVec3 *translation, SnakeQuat *rotation);

/* Where weapon `weapon`'s magazine goes this frame: 1 = relative to
 * "NPC R Hand [RHnd]" (in the gun, or in Snake's left hand during a
 * reload), 2 = relative to "NPC Spine2 [Spn2]" (holstered), 0 = not shown. */
int32_t snake_retarget_magazine_place(const SnakeRetarget *r, const SnakePlayer *p, uint32_t weapon,
                                      SnakeVec3 *translation, SnakeQuat *rotation);

int32_t snake_retarget_held_weapon(const SnakeRetarget *r, const SnakePlayer *p, SnakeVec3 *translation,
                                   SnakeQuat *rotation);

#ifdef __cplusplus
}
#endif

#endif /* SNAKE_H */
