"""Drive snake.dll through its C API from Python (ctypes), the way a host
engine would: load the runtime, give it a world with one wall via callbacks,
walk Snake into the wall, then aim and fire.

Usage (from the repo, after `cargo build --release -p snake-ffi`):
    python tools/snake_ffi_demo.py [GAME_DIR]
"""
import ctypes as C
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GAME = Path(sys.argv[1]) if len(sys.argv) > 1 else REPO.parent
DLL = REPO / "target" / "release" / ("snake.dll" if sys.platform == "win32" else "libsnake.so")


class Vec3(C.Structure):
    _fields_ = [("x", C.c_float), ("y", C.c_float), ("z", C.c_float)]


class Quat(C.Structure):
    _fields_ = [("x", C.c_float), ("y", C.c_float), ("z", C.c_float), ("w", C.c_float)]


class Input(C.Structure):
    _fields_ = [("move_x", C.c_float), ("move_y", C.c_float), ("camera_yaw", C.c_float), ("aim_pitch", C.c_float)] + [
        (n, C.c_uint8) for n in ("sprint", "walk", "crouch", "prone", "aim", "fire", "reload")
    ] + [("switch_to", C.c_int32)]


class Hit(C.Structure):
    _fields_ = [("point", Vec3), ("normal", Vec3), ("id", C.c_int64)]


RAYCAST = C.CFUNCTYPE(C.c_int32, C.c_void_p, C.POINTER(Vec3), C.POINTER(Vec3), C.POINTER(Hit))
SLIDE = C.CFUNCTYPE(None, C.c_void_p, C.POINTER(Vec3), C.POINTER(Vec3), C.c_float, C.c_float, C.POINTER(Vec3))


class World(C.Structure):
    _fields_ = [("user", C.c_void_p), ("raycast", RAYCAST), ("slide", SLIDE)]


class Shot(C.Structure):
    _fields_ = [("origin", Vec3), ("direction", Vec3), ("speed", C.c_float), ("gravity", C.c_float)]


class Status(C.Structure):
    _fields_ = [(n, C.c_int32) for n in ("stance", "gait", "aiming", "weapon", "ammo", "reserve", "reloading")] + [
        ("recoil", C.c_float)
    ]


lib = C.CDLL(str(DLL))
lib.snake_last_error.restype = C.c_char_p
lib.snake_runtime_open.restype = C.c_void_p
lib.snake_runtime_open.argtypes = [C.c_char_p, C.c_char_p]
lib.snake_bone_count.argtypes = [C.c_void_p]
lib.snake_bone_name.restype = C.c_char_p
lib.snake_bone_name.argtypes = [C.c_void_p, C.c_uint32]
lib.snake_bone_parent.argtypes = [C.c_void_p, C.c_uint32]
lib.snake_weapon_count.argtypes = [C.c_void_p]
lib.snake_player_new.restype = C.c_void_p
lib.snake_player_new.argtypes = [C.c_void_p]
lib.snake_player_update.argtypes = [C.c_void_p, C.POINTER(Input), C.POINTER(World), C.c_float]
lib.snake_player_transform.argtypes = [C.c_void_p, C.POINTER(Vec3), C.POINTER(C.c_float)]
lib.snake_player_pose.argtypes = [C.c_void_p, C.POINTER(Vec3), C.POINTER(Quat), C.c_uint32]
lib.snake_player_take_shots.argtypes = [C.c_void_p, C.POINTER(Shot), C.c_uint32]
lib.snake_player_status.argtypes = [C.c_void_p, C.POINTER(Status)]
lib.snake_player_free.argtypes = [C.c_void_p]
lib.snake_runtime_free.argtypes = [C.c_void_p]

# A world with a wall across z = 3 (front face at z = 2.9) on a floor at y = 0.
WALL_Z = 2.9


@RAYCAST
def raycast(user, a, b, hit):
    a, b = a[0], b[0]
    if a.z < WALL_Z <= b.z:
        t = (WALL_Z - a.z) / (b.z - a.z)
        hit[0].point = Vec3(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, WALL_Z)
        hit[0].normal = Vec3(0, 0, -1)
        hit[0].id = 7
        return 1
    return 0


@SLIDE
def slide(user, feet, delta, radius, height, out):
    f, d = feet[0], delta[0]
    out[0] = Vec3(f.x + d.x, f.y, min(f.z + d.z, WALL_Z - radius))


world = World(None, raycast, slide)

rt = lib.snake_runtime_open(str(GAME).encode(), str(REPO / "dictionaries").encode())
if not rt:
    sys.exit("snake_runtime_open failed: " + lib.snake_last_error().decode())
n = lib.snake_bone_count(rt)
print(f"runtime: {n} bones (bone 0 = {lib.snake_bone_name(rt, 0).decode()}), {lib.snake_weapon_count(rt)} weapons")
p = lib.snake_player_new(rt)

dt = 1 / 60


def run(seconds, **kw):
    inp = Input(switch_to=-1, **kw)
    for i in range(int(seconds / dt)):
        if not lib.snake_player_update(p, C.byref(inp), C.byref(world), C.c_float(dt)):
            sys.exit("update failed: " + lib.snake_last_error().decode())
        inp.crouch = inp.prone = inp.reload = 0
        inp.switch_to = -1


def where():
    pos, yaw = Vec3(), C.c_float()
    lib.snake_player_transform(p, C.byref(pos), C.byref(yaw))
    return pos, yaw.value


run(3.0, move_y=1.0)
pos, yaw = where()
print(f"after running at the wall: z = {pos.z:.3f} (wall face {WALL_Z}, capsule radius 0.3), heading {math.degrees(yaw):.0f} deg")

run(0.5, aim=1)
run(1.0, aim=1, fire=1)
shots = (Shot * 64)()
k = lib.snake_player_take_shots(p, shots, 64)
st = Status()
lib.snake_player_status(p, C.byref(st))
print(f"fired {k} shots; status: stance {st.stance} aiming {st.aiming} weapon {st.weapon} ammo {st.ammo}/{st.reserve}")
if k:
    s = shots[0]
    print(f"  first shot from ({s.origin.x:.2f}, {s.origin.y:.2f}, {s.origin.z:.2f}) dir ({s.direction.x:.2f}, {s.direction.y:.2f}, {s.direction.z:.2f}) at {s.speed:.0f} m/s")

positions, rotations = (Vec3 * n)(), (Quat * n)()
lib.snake_player_pose(p, positions, rotations, n)
head = next(i for i in range(n) if (lib.snake_bone_name(rt, i) or b"") == b"SKL_004_HEAD")
print(f"head at ({positions[head].x:.2f}, {positions[head].y:.2f}, {positions[head].z:.2f}) in root space")

lib.snake_player_free(p)
lib.snake_runtime_free(rt)
print("ok")
