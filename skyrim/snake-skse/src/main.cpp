// SnakeSKSE: brings Snake's player systems (snake.dll, built from this repo's
// Rust crates) into Skyrim Special Edition through SKSE.
//
// Phase B: Snake's animation drives the player's skeleton. Press F8 in game
// to toggle. Skyrim still moves the character; Snake's controller picks
// walk / run / dash from the player's actual speed, C and Z toggle crouch
// and prone, the right mouse button raises the weapon. Every frame, after
// Skyrim's own animation update, Snake's pose is retargeted onto the
// skeleton (snake_retarget_apply) and written into the bone nodes.
//
// Game structures are accessed by offset for runtime 1.7.104 (layouts from
// SKSE 2.3.1's headers; virtual slots cross-checked with CommonLibSSE-NG).
// Log: Documents/My Games/Skyrim Special Edition/SKSE/SnakeSKSE.log.

#include <windows.h>
#include <shlobj.h>
#include <share.h>

#include <chrono>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

#include "skse64/PluginAPI.h"
#include "skse64_common/skse_version.h"

#include "snake.h"

namespace {

// ---- logging / paths -------------------------------------------------------

HMODULE g_self = nullptr;
FILE* g_log = nullptr;
PluginHandle g_handle = kPluginHandle_Invalid;
SKSEMessagingInterface* g_messaging = nullptr;

void Log(const char* fmt, ...) {
    if (!g_log) return;
    va_list args;
    va_start(args, fmt);
    vfprintf(g_log, fmt, args);
    va_end(args);
    fputc('\n', g_log);
    fflush(g_log);
}

void OpenLog() {
    PWSTR docs = nullptr;
    if (FAILED(SHGetKnownFolderPath(FOLDERID_Documents, 0, nullptr, &docs))) return;
    std::wstring path = std::wstring(docs) + L"\\My Games\\Skyrim Special Edition\\SKSE";
    CoTaskMemFree(docs);
    CreateDirectoryW(path.c_str(), nullptr);
    path += L"\\SnakeSKSE.log";
    // Shared so the log can be read while the game runs.
    g_log = _wfsopen(path.c_str(), L"w", _SH_DENYNO);
}

std::wstring PluginDir() {
    wchar_t buf[MAX_PATH];
    DWORD n = GetModuleFileNameW(g_self, buf, MAX_PATH);
    std::wstring p(buf, n);
    return p.substr(0, p.find_last_of(L"\\/"));
}

std::wstring Parent(const std::wstring& p) { return p.substr(0, p.find_last_of(L"\\/")); }

std::string Narrow(const std::wstring& w) {
    int n = WideCharToMultiByte(CP_UTF8, 0, w.c_str(), -1, nullptr, 0, nullptr, nullptr);
    std::string s(n > 0 ? n - 1 : 0, '\0');
    if (n > 0) WideCharToMultiByte(CP_UTF8, 0, w.c_str(), -1, s.data(), n, nullptr, nullptr);
    return s;
}

std::string IniString(const std::wstring& ini, const wchar_t* key) {
    wchar_t buf[1024];
    GetPrivateProfileStringW(L"Snake", key, L"", buf, 1024, ini.c_str());
    return Narrow(buf);
}

// ---- snake.dll ---------------------------------------------------------------

struct SnakeApi {
    decltype(&snake_last_error) last_error = nullptr;
    decltype(&snake_runtime_open) runtime_open = nullptr;
    decltype(&snake_bone_count) bone_count = nullptr;
    decltype(&snake_bone_name) bone_name = nullptr;
    decltype(&snake_weapon_count) weapon_count = nullptr;
    decltype(&snake_player_new) player_new = nullptr;
    decltype(&snake_player_update) player_update = nullptr;
    decltype(&snake_player_status) player_status = nullptr;
    decltype(&snake_player_transform) player_transform = nullptr;
    decltype(&snake_retarget_new) retarget_new = nullptr;
    decltype(&snake_retarget_count) retarget_count = nullptr;
    decltype(&snake_retarget_bone) retarget_bone = nullptr;
    decltype(&snake_retarget_apply) retarget_apply = nullptr;
} g_snake;

SnakeRuntime* g_runtime = nullptr;
SnakePlayer* g_player = nullptr;
SnakeRetarget* g_retarget = nullptr;
std::vector<std::string> g_boneNames;

bool LoadSnakeDll() {
    std::wstring dll = PluginDir() + L"\\snake.dll";
    HMODULE h = LoadLibraryW(dll.c_str());
    if (!h) {
        Log("could not load %s (error %lu)", Narrow(dll).c_str(), GetLastError());
        return false;
    }
#define RESOLVE(field, name)                                                                \
    g_snake.field = reinterpret_cast<decltype(g_snake.field)>(GetProcAddress(h, name));     \
    if (!g_snake.field) {                                                                   \
        Log("snake.dll is missing %s", name);                                               \
        return false;                                                                       \
    }
    RESOLVE(last_error, "snake_last_error");
    RESOLVE(runtime_open, "snake_runtime_open");
    RESOLVE(bone_count, "snake_bone_count");
    RESOLVE(bone_name, "snake_bone_name");
    RESOLVE(weapon_count, "snake_weapon_count");
    RESOLVE(player_new, "snake_player_new");
    RESOLVE(player_update, "snake_player_update");
    RESOLVE(player_status, "snake_player_status");
    RESOLVE(player_transform, "snake_player_transform");
    RESOLVE(retarget_new, "snake_retarget_new");
    RESOLVE(retarget_count, "snake_retarget_count");
    RESOLVE(retarget_bone, "snake_retarget_bone");
    RESOLVE(retarget_apply, "snake_retarget_apply");
#undef RESOLVE
    Log("loaded %s", Narrow(dll).c_str());
    return true;
}

bool OpenRuntime() {
    std::wstring ini = PluginDir() + L"\\snake.ini";
    std::string game = IniString(ini, L"MgsvDir");
    std::string dicts = IniString(ini, L"DictionariesDir");
    if (game.empty() || dicts.empty()) {
        Log("set MgsvDir and DictionariesDir in %s", Narrow(ini).c_str());
        return false;
    }
    auto t0 = std::chrono::steady_clock::now();
    g_runtime = g_snake.runtime_open(game.c_str(), dicts.c_str());
    if (!g_runtime) {
        Log("snake_runtime_open failed: %s", g_snake.last_error());
        return false;
    }
    g_player = g_snake.player_new(g_runtime);
    if (!g_player) {
        Log("snake_player_new failed: %s", g_snake.last_error());
        return false;
    }
    // Plugins live in <Skyrim>/Data/SKSE/Plugins.
    std::string skyrim = Narrow(Parent(Parent(Parent(PluginDir()))));
    g_retarget = g_snake.retarget_new(g_runtime, skyrim.c_str());
    if (!g_retarget) {
        Log("snake_retarget_new failed: %s", g_snake.last_error());
        return false;
    }
    uint32_t n = g_snake.retarget_count(g_retarget);
    for (uint32_t i = 0; i < n; ++i) g_boneNames.emplace_back(g_snake.retarget_bone(g_retarget, i));
    double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    Log("runtime ready in %.1fs: %u Snake bones, %u weapons, %u Skyrim bones retargeted (skeleton from %s)", secs,
        g_snake.bone_count(g_runtime), g_snake.weapon_count(g_runtime), n, skyrim.c_str());
    return true;
}

// ---- game structures (runtime 1.7.104) --------------------------------------

constexpr uintptr_t kPlayerSingleton = 0x03230778;  // PlayerCharacter** (SKSE g_thePlayer)
constexpr size_t kVtUpdateAnimation = 0x7D;          // TESObjectREFR::UpdateAnimation(float)
constexpr size_t kVtGet3D1 = 0x6F;                   // TESObjectREFR::Get3D1(bool firstPerson)
constexpr size_t kVtAsNode = 0x03;                   // NiObject::AsNode()
constexpr size_t kRefPosition = 0x4C;                // TESObjectREFR data.location (NiPoint3)

// NiAVObject / NiNode fields.
constexpr size_t kName = 0x10;
constexpr size_t kLocalRot = 0x48;   // NiMatrix3, row-major, v' = M v
constexpr size_t kLocalPos = 0x6C;
constexpr size_t kLocalScale = 0x78;
constexpr size_t kWorldRot = 0x7C;
constexpr size_t kWorldPos = 0xA0;
constexpr size_t kWorldScale = 0xAC;
constexpr size_t kParent = 0x30;
constexpr size_t kChildren = 0x118;   // NiTArray data pointer
constexpr size_t kChildCapacity = 0x120; // NiTArray capacity (slots may be empty)

template <class T>
T& At(void* base, size_t off) {
    return *reinterpret_cast<T*>(reinterpret_cast<char*>(base) + off);
}

void** Vtable(void* obj) { return *reinterpret_cast<void***>(obj); }

void* ThePlayer() {
    auto base = reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr));
    return *reinterpret_cast<void**>(base + kPlayerSingleton);
}

void* Get3D(void* ref, bool firstPerson) {
    using Fn = void* (*)(void*, bool);
    return reinterpret_cast<Fn>(Vtable(ref)[kVtGet3D1])(ref, firstPerson);
}

void* AsNode(void* obj) {
    using Fn = void* (*)(void*);
    return reinterpret_cast<Fn>(Vtable(obj)[kVtAsNode])(obj);
}

const char* Name(void* obj) { return At<const char*>(obj, kName); }

template <class F>
void ForEachChild(void* node, F&& f) {
    void** data = At<void**>(node, kChildren);
    uint16_t n = At<uint16_t>(node, kChildCapacity);
    if (!data || n > 512) return;
    for (uint16_t i = 0; i < n; ++i)
        if (data[i]) f(data[i]);
}

void* FindNode(void* obj, const std::string& name) {
    const char* n = Name(obj);
    if (n && name == n) return obj;
    void* node = AsNode(obj);
    if (!node) return nullptr;
    void* found = nullptr;
    ForEachChild(node, [&](void* c) {
        if (!found) found = FindNode(c, name);
    });
    return found;
}

// 3x3 helpers (row-major, column vectors).
void MatMul(const float* a, const float* b, float* out) {
    for (int r = 0; r < 3; ++r)
        for (int c = 0; c < 3; ++c)
            out[r * 3 + c] = a[r * 3] * b[c] + a[r * 3 + 1] * b[3 + c] + a[r * 3 + 2] * b[6 + c];
}

void MatVec(const float* m, const float* v, float* out) {
    for (int r = 0; r < 3; ++r) out[r] = m[r * 3] * v[0] + m[r * 3 + 1] * v[1] + m[r * 3 + 2] * v[2];
}

void QuatToMat(const SnakeQuat& q, float* m) {
    float x = q.x, y = q.y, z = q.z, w = q.w;
    m[0] = 1 - 2 * (y * y + z * z); m[1] = 2 * (x * y - w * z);     m[2] = 2 * (x * z + w * y);
    m[3] = 2 * (x * y + w * z);     m[4] = 1 - 2 * (x * x + z * z); m[5] = 2 * (y * z - w * x);
    m[6] = 2 * (x * z - w * y);     m[7] = 2 * (y * z + w * x);     m[8] = 1 - 2 * (x * x + y * y);
}

// ---- per-frame hook --------------------------------------------------------------

using UpdateAnimationFn = void (*)(void* self, float delta);
UpdateAnimationFn g_originalUpdateAnimation = nullptr;
std::mutex g_mutex;
bool g_enabled = false;
bool g_f8Down = false, g_cDown = false, g_zDown = false;
float g_lastPos[3] = {0, 0, 0};
bool g_havePos = false;
void* g_cachedRoot = nullptr;
std::vector<void*> g_nodes;
double g_logTimer = 0;
int g_frames = 0;

bool KeyPressed(int vk, bool& wasDown) {
    bool down = (GetAsyncKeyState(vk) & 0x8000) != 0;
    bool pressed = down && !wasDown;
    wasDown = down;
    return pressed;
}

void ApplySnake(void* player, float delta) {
    void* root = Get3D(player, false);
    if (!root) return;
    if (root != g_cachedRoot) {
        g_nodes.clear();
        int missing = 0;
        for (auto& n : g_boneNames) {
            g_nodes.push_back(FindNode(root, n));
            missing += g_nodes.back() == nullptr;
        }
        g_cachedRoot = root;
        Log("player 3D %p (%s): found %zu of %zu retarget bones", root, Name(root) ? Name(root) : "?",
            g_nodes.size() - missing, g_nodes.size());
        for (size_t i = 0; i < g_nodes.size(); ++i)
            if (!g_nodes[i]) Log("  missing: %s", g_boneNames[i].c_str());
        g_frames = 0;
    }

    // Snake's input from what the player is doing.
    float* pos = &At<float>(player, kRefPosition);
    float speed = 0;
    if (g_havePos && delta > 0) {
        float dx = pos[0] - g_lastPos[0], dy = pos[1] - g_lastPos[1];
        speed = std::sqrt(dx * dx + dy * dy) / delta / 70.0f;  // m/s
    }
    memcpy(g_lastPos, pos, sizeof g_lastPos);
    g_havePos = true;

    SnakeInput in{};
    in.switch_to = -1;
    in.move_y = speed > 0.3f ? 1.0f : 0.0f;
    in.walk = speed > 0.3f && speed < 2.6f;
    in.sprint = speed > 5.5f;
    in.crouch = KeyPressed('C', g_cDown);
    in.prone = KeyPressed('Z', g_zDown);
    in.aim = (GetAsyncKeyState(VK_RBUTTON) & 0x8000) != 0;
    in.fire = in.aim && (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0;
    // Keep Snake's own heading as "forward": Skyrim decides where the actor faces.
    SnakeVec3 snakePos{};
    float snakeYaw = 0;
    g_snake.player_transform(g_player, &snakePos, &snakeYaw);
    in.camera_yaw = snakeYaw;
    g_snake.player_update(g_player, &in, nullptr, delta);

    uint32_t n = static_cast<uint32_t>(g_nodes.size());
    std::vector<SnakeQuat> rot(n);
    std::vector<SnakeVec3> tr(n);
    std::vector<uint8_t> hasTr(n);
    if (g_snake.retarget_apply(g_retarget, g_player, rot.data(), tr.data(), hasTr.data(), n) != n) return;
    for (uint32_t i = 0; i < n; ++i) {
        void* node = g_nodes[i];
        if (!node) continue;
        QuatToMat(rot[i], &At<float>(node, kLocalRot));
        if (hasTr[i]) {
            float* lp = &At<float>(node, kLocalPos);
            lp[0] = tr[i].x;
            lp[1] = tr[i].y;
            lp[2] = tr[i].z;
        }
    }
    // World transforms of the retargeted bones, parent-first (table order),
    // from each bone's actual parent node.
    for (uint32_t i = 0; i < n; ++i) {
        void* node = g_nodes[i];
        if (!node) continue;
        void* parent = At<void*>(node, kParent);
        if (!parent) continue;
        float prot[9], ppos[3];
        memcpy(prot, &At<float>(parent, kWorldRot), sizeof prot);
        memcpy(ppos, &At<float>(parent, kWorldPos), sizeof ppos);
        float ps = At<float>(parent, kWorldScale);
        float* lr = &At<float>(node, kLocalRot);
        float* lp = &At<float>(node, kLocalPos);
        float scaled[3] = {lp[0] * ps, lp[1] * ps, lp[2] * ps}, rotated[3];
        MatVec(prot, scaled, rotated);
        MatMul(prot, lr, &At<float>(node, kWorldRot));
        float* wp = &At<float>(node, kWorldPos);
        for (int k = 0; k < 3; ++k) wp[k] = ppos[k] + rotated[k];
        At<float>(node, kWorldScale) = ps * At<float>(node, kLocalScale);
    }
    if (g_frames++ < 3) Log("frame %d: dt %.4f speed %.2f m/s", g_frames, delta, speed);
    g_logTimer += delta;
    if (g_logTimer > 2.0) {
        g_logTimer = 0;
        SnakeStatus st{};
        g_snake.player_status(g_player, &st);
        Log("snake: speed %.2f m/s stance %d gait %d aiming %d", speed, st.stance, st.gait, st.aiming);
    }
}

// SEH guard: a fault inside Snake's per-frame code turns Snake mode off
// instead of crashing the game. (No C++ objects in this frame.)
bool SafeApply(void* self, float delta) {
    __try {
        ApplySnake(self, delta);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

void HookedUpdateAnimation(void* self, float delta) {
    g_originalUpdateAnimation(self, delta);
    if (self != ThePlayer() || !g_retarget) return;
    std::lock_guard<std::mutex> lock(g_mutex);
    if (KeyPressed(VK_F8, g_f8Down)) {
        g_enabled = !g_enabled;
        g_havePos = false;
        Log("Snake mode %s", g_enabled ? "ON" : "OFF");
    }
    if (g_enabled && !SafeApply(self, delta)) {
        g_enabled = false;
        g_cachedRoot = nullptr;
        Log("fault in Snake mode; turned it off (please send this log)");
    }
}

bool InstallHook() {
    void* player = ThePlayer();
    if (!player) {
        Log("no player yet");
        return false;
    }
    void** vt = Vtable(player);
    DWORD old;
    if (!VirtualProtect(&vt[kVtUpdateAnimation], sizeof(void*), PAGE_EXECUTE_READWRITE, &old)) {
        Log("VirtualProtect failed");
        return false;
    }
    g_originalUpdateAnimation = reinterpret_cast<UpdateAnimationFn>(vt[kVtUpdateAnimation]);
    vt[kVtUpdateAnimation] = reinterpret_cast<void*>(&HookedUpdateAnimation);
    VirtualProtect(&vt[kVtUpdateAnimation], sizeof(void*), old, &old);
    Log("hooked PlayerCharacter::UpdateAnimation (vtable %p, original %p); press F8 in game for Snake mode", vt,
        g_originalUpdateAnimation);
    return true;
}

void OnMessage(SKSEMessagingInterface::Message* msg) {
    if (msg->type == SKSEMessagingInterface::kMessage_DataLoaded) {
        Log("Skyrim data loaded");
        if (LoadSnakeDll() && OpenRuntime()) InstallHook();
    }
}

}  // namespace

extern "C" {

__declspec(dllexport) SKSEPluginVersionData SKSEPlugin_Version = {
    SKSEPluginVersionData::kVersion,
    2,
    "SnakeSKSE",
    "mgsv-rust-engine",
    "",
    0,  // versionIndependenceEx
    0,  // versionIndependence: built against SKSE's own headers for this runtime
    {RUNTIME_VERSION_1_7_104, 0},
    0,
};

__declspec(dllexport) bool SKSEPlugin_Load(const SKSEInterface* skse) {
    OpenLog();
    Log("SnakeSKSE loading (runtime %08X)", skse->runtimeVersion);
    g_handle = skse->GetPluginHandle();
    g_messaging = static_cast<SKSEMessagingInterface*>(skse->QueryInterface(kInterface_Messaging));
    if (!g_messaging) {
        Log("no messaging interface");
        return false;
    }
    g_messaging->RegisterListener(g_handle, "SKSE", OnMessage);
    return true;
}

}  // extern "C"

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) g_self = module;
    return TRUE;
}
