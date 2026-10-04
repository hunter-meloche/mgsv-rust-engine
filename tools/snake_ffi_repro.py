"""Replay what SnakeSKSE does each frame (idle input, camera yaw = Snake's own
heading, ~60 Hz, retarget every frame) through snake.dll, printing timings.
Used to reproduce in-game hangs outside Skyrim.

    python tools/snake_ffi_repro.py [FRAMES] [SKYRIM_DIR]
"""
import ctypes as C
import sys
import time
from pathlib import Path

FRAMES = int(sys.argv[1]) if len(sys.argv) > 1 else 300
SKYRIM = sys.argv[2] if len(sys.argv) > 2 else r"H:\SteamLibrary\steamapps\common\Skyrim Special Edition"
sys.argv = sys.argv[:1]  # the demo module reads argv for the game dir

sys.path.insert(0, str(Path(__file__).parent))
from snake_ffi_demo import GAME, REPO, Input, Quat, Status, Vec3, lib  # noqa: E402

lib.snake_retarget_new.restype = C.c_void_p
lib.snake_retarget_new.argtypes = [C.c_void_p, C.c_char_p]
lib.snake_retarget_count.argtypes = [C.c_void_p]
lib.snake_retarget_apply.argtypes = [C.c_void_p, C.c_void_p, C.POINTER(Quat), C.POINTER(Vec3), C.POINTER(C.c_uint8), C.c_uint32]

rt = lib.snake_runtime_open(str(GAME).encode(), str(REPO / "dictionaries").encode())
p = lib.snake_player_new(rt)
rg = lib.snake_retarget_new(rt, SKYRIM.encode())
if not rg:
    sys.exit("retarget: " + lib.snake_last_error().decode())
n = lib.snake_retarget_count(rg)
rot, tr, has = (Quat * n)(), (Vec3 * n)(), (C.c_uint8 * n)()
for f in range(FRAMES):
    pos, yaw = Vec3(), C.c_float()
    lib.snake_player_transform(p, C.byref(pos), C.byref(yaw))
    inp = Input(switch_to=-1, camera_yaw=yaw.value)
    t0 = time.perf_counter()
    print(f"frame {f}: update...", end="", flush=True)
    ok = lib.snake_player_update(p, C.byref(inp), None, C.c_float(0.0166))
    t1 = time.perf_counter()
    k = lib.snake_retarget_apply(rg, p, rot, tr, has, n)
    t2 = time.perf_counter()
    print(f" ok={ok} {1000 * (t1 - t0):.2f} ms, retarget {k} {1000 * (t2 - t1):.2f} ms, yaw {yaw.value:.3f}", flush=True)
print("done")
