// SnakeSKSE: brings Snake's player systems (snake.dll, built from this repo's
// Rust crates) into Skyrim Special Edition through SKSE.
//
// Phase B: Snake's animation drives the player's skeleton. Press F8 in game
// (F10 cycles write modes: compute only / local transforms / + world)
// to toggle. Skyrim still moves the character; Snake follows Skyrim's own
// state: walk / run / sprint flags, crouch while sneaking, X toggles prone,
// the right mouse button raises the weapon. Every frame, after
// Skyrim's own animation update, Snake's pose is retargeted onto the
// skeleton (snake_retarget_apply) and written into the bone nodes.
//
// Game structures are accessed by offset for runtime 1.7.104 (layouts from
// SKSE 2.3.1's headers; virtual slots cross-checked with CommonLibSSE-NG).
// Log: Documents/My Games/Skyrim Special Edition/SKSE/SnakeSKSE.log.

#include <windows.h>
#include <shlobj.h>
#include <xaudio2.h>
#include <share.h>
#include <malloc.h>

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <functional>
#include <atomic>
#include <map>
#include <mutex>
#include <thread>
#include <string>
#include <unordered_map>
#include <vector>

#include "skse64/PluginAPI.h"
#include "skse64/gamethreads.h"
#include "skse64/Hooks_UI.h"
#include "skse64/ScaleformMovie.h"
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
    decltype(&snake_weapon_mesh) weapon_mesh = nullptr;
    decltype(&snake_weapon_muzzle) weapon_muzzle = nullptr;
    decltype(&snake_weapon_name) weapon_name = nullptr;
    decltype(&snake_weapon_magazine_mesh) weapon_magazine_mesh = nullptr;
    decltype(&snake_weapon_holster_shape) weapon_holster_shape = nullptr;
    decltype(&snake_player_set_unlimited_magazines) player_set_unlimited_magazines = nullptr;
    decltype(&snake_player_set_tempo) player_set_tempo = nullptr;
    decltype(&snake_player_foley) player_foley = nullptr;
    decltype(&snake_player_clip) player_clip = nullptr;
    decltype(&snake_retarget_magazine_place) retarget_magazine_place = nullptr;
    decltype(&snake_player_new) player_new = nullptr;
    decltype(&snake_player_update) player_update = nullptr;
    decltype(&snake_player_status) player_status = nullptr;
    decltype(&snake_player_transform) player_transform = nullptr;
    decltype(&snake_player_take_shots) player_take_shots = nullptr;
    decltype(&snake_retarget_new) retarget_new = nullptr;
    decltype(&snake_retarget_count) retarget_count = nullptr;
    decltype(&snake_retarget_bone) retarget_bone = nullptr;
    decltype(&snake_retarget_apply) retarget_apply = nullptr;
    decltype(&snake_retarget_weapon_place) retarget_weapon_place = nullptr;
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
    RESOLVE(weapon_mesh, "snake_weapon_mesh");
    RESOLVE(weapon_muzzle, "snake_weapon_muzzle");
    RESOLVE(weapon_name, "snake_weapon_name");
    RESOLVE(weapon_magazine_mesh, "snake_weapon_magazine_mesh");
    RESOLVE(weapon_holster_shape, "snake_weapon_holster_shape");
    RESOLVE(player_set_unlimited_magazines, "snake_player_set_unlimited_magazines");
    RESOLVE(player_set_tempo, "snake_player_set_tempo");
    RESOLVE(player_foley, "snake_player_foley");
    RESOLVE(player_clip, "snake_player_clip");
    RESOLVE(retarget_magazine_place, "snake_retarget_magazine_place");
    RESOLVE(player_new, "snake_player_new");
    RESOLVE(player_update, "snake_player_update");
    RESOLVE(player_status, "snake_player_status");
    RESOLVE(player_transform, "snake_player_transform");
    RESOLVE(player_take_shots, "snake_player_take_shots");
    RESOLVE(retarget_new, "snake_retarget_new");
    RESOLVE(retarget_count, "snake_retarget_count");
    RESOLVE(retarget_bone, "snake_retarget_bone");
    RESOLVE(retarget_apply, "snake_retarget_apply");
    RESOLVE(retarget_weapon_place, "snake_retarget_weapon_place");
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
    // snake.ini UnlimitedMagazines (default on): reloads never run out.
    int unlimited = GetPrivateProfileIntW(L"Snake", L"UnlimitedMagazines", 1, ini.c_str());
    g_snake.player_set_unlimited_magazines(g_player, unlimited);
    Log("magazines: %s", unlimited ? "unlimited" : "from the reserve");
    // snake.ini MoveSpeedScale (percent, default 150): the tempo of all of
    // Snake's motion; moving, turning, stances and reloads speed up together.
    float tempo = GetPrivateProfileIntW(L"Snake", L"MoveSpeedScale", 150, ini.c_str()) / 100.0f;
    g_snake.player_set_tempo(g_player, tempo);
    Log("tempo: %.0f%% of MGSV's", tempo * 100.0f);
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
constexpr size_t kVtAsGeometry = 0x07;               // NiObject::AsGeometry()
constexpr size_t kGeomSkinInstance = 0x130;          // BSGeometry runtime data + 0x10
constexpr size_t kSkinBones = 0x28;                  // NiSkinInstance::bones (NiAVObject**)
constexpr size_t kSkinBoneCount = 0x3C;              // NiSkinInstance::numMatrices
// TESObjectREFR data (OBJ_REFR) sits 8 bytes later than in SE on this
// runtime (verified in game): angle (pitch, roll, heading) at 0x48,
// location at 0x54.
constexpr size_t kRefAngle = 0x48;
constexpr size_t kRefPosition = 0x54;

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

void* AsGeometry(void* obj) {
    using Fn = void* (*)(void*);
    return reinterpret_cast<Fn>(Vtable(obj)[kVtAsGeometry])(obj);
}

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

// ---- state ---------------------------------------------------------------------

using UpdateAnimationFn = void (*)(void* self, float delta);
UpdateAnimationFn g_originalUpdateAnimation = nullptr;
std::vector<void*> g_nodes;
bool g_layoutOk = false;
// The layout check passed once this session: the memory layout it guards is
// the runtime's, so later skeletons (after loading a save, whose world
// transforms Skyrim has not updated yet) need not pass again.
bool g_layoutVerified = false;
int g_layoutRetries = 0;  // checks of the current skeleton so far

bool Readable(const void* p, size_t n) {
    MEMORY_BASIC_INFORMATION mbi{};
    if (!p || !VirtualQuery(p, &mbi, sizeof mbi)) return false;
    if (mbi.State != MEM_COMMIT || (mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD))) return false;
    return reinterpret_cast<const char*>(p) + n <= reinterpret_cast<const char*>(mbi.BaseAddress) + mbi.RegionSize;
}

// MSVC RTTI class name of a polymorphic object (".?AVPlayerCharacter@@"),
// or null: vtable[-1] is the complete object locator, whose type
// descriptor (an image-relative offset) holds the decorated name at +0x10.
const char* RttiName(const void* obj) {
    static const uintptr_t base = reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr));
    static const size_t size = [] {
        auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
        return static_cast<size_t>(reinterpret_cast<IMAGE_NT_HEADERS64*>(base + dos->e_lfanew)->OptionalHeader.SizeOfImage);
    }();
    auto in = [&](uintptr_t p) { return p >= base && p < base + size; };
    if (!obj || !Readable(obj, 8)) return nullptr;
    auto vt = *reinterpret_cast<const uintptr_t* const*>(obj);
    if (!in(reinterpret_cast<uintptr_t>(vt)) || !Readable(vt - 1, 8)) return nullptr;
    auto col = reinterpret_cast<const uint32_t*>(vt[-1]);
    if (!in(reinterpret_cast<uintptr_t>(col)) || col[0] != 1) return nullptr;
    uintptr_t td = base + col[3];
    if (!in(td) || !Readable(reinterpret_cast<const void*>(td + 0x10), 4)) return nullptr;
    const char* name = reinterpret_cast<const char*>(td + 0x10);
    return name[0] == '.' && name[1] == '?' ? name : nullptr;
}

// Diagnostics: the player's character controller. Actor::currentProcess
// (AIProcess, no RTTI) sits at +0xF0 in SE (this runtime shifts reference
// data by 8); AIProcess+0x10 is its MiddleHighProcessData, which holds the
// controller; the classes are checked by RTTI name.
void ProbeCharController(void* player) {
    for (size_t pOff : {size_t(0xF0), size_t(0xF8)}) {
        void* process = At<void*>(player, pOff);
        if (!process || !Readable(process, 0x40)) continue;
        for (size_t mOff = 0x08; mOff <= 0x20; mOff += 8) {
            void* mh = At<void*>(process, mOff);
            if (!mh || !Readable(mh, 0x400)) continue;
            for (size_t off = 0; off < 0x400; off += 8) {
                void* q = At<void*>(mh, off);
                if (!q || (reinterpret_cast<uintptr_t>(q) & 7) || !Readable(q, 0x40)) continue;
                const char* n = RttiName(q);
                if (n && (strstr(n, "bhk") || strstr(n, "hkp")))
                    Log("probe: player+%zX -> process %p +%zX -> %p +%zX -> %p %s", pOff, process, mOff, mh, off, q, n);
            }
        }
    }
}

// The player's bhkCharProxyController (RTTI-checked), or null.
void* CharController(void* player) {
    void* process = player && Readable(player, 0x100) ? At<void*>(player, 0xF8) : nullptr;
    void* mh = process && Readable(process, 0x10) ? At<void*>(process, 0x08) : nullptr;
    void* cc = mh && Readable(mh, 0x258) ? At<void*>(mh, 0x250) : nullptr;
    const char* n = cc ? RttiName(cc) : nullptr;
    return n && strstr(n, "bhkChar") ? cc : nullptr;
}

// Diagnostics: the controller's memory as rows of four floats.
void DumpCharController(void* player, const char* label) {
    void* cc = CharController(player);
    if (!cc || !Readable(cc, 0x400)) {
        Log("probe %s: no character controller", label);
        return;
    }
    Log("probe %s: controller %p", label, cc);
    for (size_t off = 0; off < 0x400; off += 16) {
        const float* f = &At<float>(cc, off);
        bool interesting = false;
        for (int k = 0; k < 4; ++k)
            if (std::isfinite(f[k]) && std::fabs(f[k]) > 1e-4f && std::fabs(f[k]) < 1e5f) interesting = true;
        if (interesting) Log("  +%03zX  %10.4f %10.4f %10.4f %10.4f", off, f[0], f[1], f[2], f[3]);
    }
}

class DumpTask : public TaskDelegate {
public:
    explicit DumpTask(std::string label) : label_(std::move(label)) {}
    void Run() override { DumpCharController(ThePlayer(), label_.c_str()); }
    void Dispose() override { delete this; }

private:
    std::string label_;
};

float SafeGetAv(void* actor, uint32_t av);

// Diagnostics: camera nodes in a 3D tree (name, parent, local position).
void LogCameraNodes(void* obj, const char* view, int depth) {
    if (!obj || depth > 40 || !Readable(obj, 0x140)) return;
    const char* n = Name(obj);
    if (n && Readable(n, 1) && (strstr(n, "Cam") || strstr(n, "cam"))) {
        void* parent = At<void*>(obj, kParent);
        const float* lp = &At<float>(obj, kLocalPos);
        const float* wp = &At<float>(obj, kWorldPos);
        Log("probe %s: node %s (parent %s) local %.1f %.1f %.1f world z %.1f", view, n,
            parent && Name(parent) ? Name(parent) : "?", lp[0], lp[1], lp[2], wp[2]);
    }
    if (void* node = AsNode(obj)) ForEachChild(node, [&](void* c) { LogCameraNodes(c, view, depth + 1); });
}

class CameraProbeTask : public TaskDelegate {
public:
    void Run() override {
        void* player = ThePlayer();
        if (!player) return;
        for (uint32_t av = 13; av <= 17; ++av) Log("probe: actor value %u = %.1f", av, SafeGetAv(player, av));
        LogCameraNodes(Get3D(player, false), "3rd", 0);
        LogCameraNodes(Get3D(player, true), "1st", 0);
        Log("probe: player z %.1f", At<float>(player, kRefPosition + 8));
    }
    void Dispose() override { delete this; }
};

// Diagnostics (probe=6): the player object's bytes, and those of the
// objects its first 0x400 bytes point at, to snake_probe_<n>.bin, on the
// main thread (to diff states such as sneaking).
class MemDumpTask : public TaskDelegate {
public:
    void Run() override {
        static int n = 0;
        void* player = ThePlayer();
        if (!player) return;
        std::wstring path = PluginDir() + L"\\snake_probe_" + std::to_wstring(n++) + L".bin";
        FILE* f = nullptr;
        if (_wfopen_s(&f, path.c_str(), L"wb") != 0 || !f) return;
        constexpr size_t kSize = 0x1400;
        std::vector<uint8_t> buf(kSize, 0xCD);
        auto copy = [&](const void* src) {
            std::fill(buf.begin(), buf.end(), 0xCD);
            for (size_t off = 0; off < kSize; off += 0x100)
                if (src && Readable(static_cast<const char*>(src) + off, 0x100)) memcpy(&buf[off], static_cast<const char*>(src) + off, 0x100);
            fwrite(buf.data(), 1, kSize, f);
        };
        copy(player);
        // Pointed-at objects: the pointers in the player's first 0x400 bytes.
        for (size_t off = 0; off < 0x400; off += 8) {
            void* q = At<void*>(player, off);
            copy(Readable(q, 0x100) ? q : nullptr);
        }
        // And one level further below the AI process (player +0xF8): its first 0x400 bytes' pointers.
        void* process = At<void*>(player, 0xF8);
        for (size_t off = 0; off < 0x400; off += 8) {
            void* q = process && Readable(process, 0x400) ? At<void*>(process, off) : nullptr;
            copy(Readable(q, 0x100) ? q : nullptr);
        }
        void* middle = process && Readable(process, 0x10) ? At<void*>(process, 0x08) : nullptr;
        for (size_t off = 0; off < 0x400; off += 8) {
            void* q = middle && Readable(middle, 0x400) ? At<void*>(middle, off) : nullptr;
            copy(Readable(q, 0x100) ? q : nullptr);
        }
        fclose(f);
        Log("probe: player memory dumped (%d)", n - 1);
    }
    void Dispose() override { delete this; }
};

class ProbeTask : public TaskDelegate {
public:
    void Run() override {
        void* player = ThePlayer();
        if (!player) return;
        Log("probe: player %p %s", player, RttiName(player) ? RttiName(player) : "?");
        ProbeCharController(player);
        Log("probe: done");
    }
    void Dispose() override { delete this; }
};

// Bones referenced by skinned geometry under `obj` (name -> node). Skyrim
// keeps some bones (fingers, toes, twists) out of the skeleton's child
// arrays at runtime, but the skin instances still point at them.
void CollectSkinBones(void* obj, std::unordered_map<std::string, void*>& out, int depth = 0) {
    if (!obj || depth > 64 || !Readable(obj, 0x140)) return;
    if (void* node = AsNode(obj)) {
        ForEachChild(node, [&](void* c) { CollectSkinBones(c, out, depth + 1); });
        return;
    }
    void* geom = AsGeometry(obj);
    if (!geom) return;
    void* skin = At<void*>(geom, kGeomSkinInstance);
    if (!skin || !Readable(skin, 0x48)) return;
    void** bones = At<void**>(skin, kSkinBones);
    uint32_t n = At<uint32_t>(skin, kSkinBoneCount);
    if (!bones || n > 512 || !Readable(bones, n * sizeof(void*))) return;
    for (uint32_t i = 0; i < n; ++i) {
        void* b = bones[i];
        if (!b || !Readable(b, 0x18)) continue;
        const char* nm = Name(b);
        if (nm && Readable(nm, 1)) out.emplace(nm, b);
    }
}

// Inventory of a 3D tree: node count, geometry count, and names matching
// finger / toe / twist bones (diagnostics for the missing bones).
void Inventory(void* obj, const char* label, int depth, int& nodes, int& geoms, std::string& hits) {
    if (!obj || depth > 64 || !Readable(obj, 0x140)) return;
    const char* nm = Name(obj);
    if (nm && Readable(nm, 1) && (strstr(nm, "Finger") || strstr(nm, "Toe") || strstr(nm, "Twist")) && hits.size() < 600)
        hits += std::string(" ") + nm;
    if (void* node = AsNode(obj)) {
        ++nodes;
        ForEachChild(node, [&](void* c) { Inventory(c, label, depth + 1, nodes, geoms, hits); });
    } else if (AsGeometry(obj)) {
        ++geoms;
    }
}

// Raw dump of a node's child array (to check the layout assumptions).
void DumpChildren(void* obj) {
    if (!obj || !Readable(obj, 0x130)) return;
    void* node = AsNode(obj);
    uint16_t* f = &At<uint16_t>(obj, 0x120);
    void** data = At<void**>(obj, kChildren);
    Log("  dump %s: AsNode %p data %p u16[0x120..0x126] = %u %u %u %u", Name(obj), node, data, f[0], f[1], f[2], f[3]);
    for (int i = 0; i < 12 && data && Readable(data + i, sizeof(void*)); ++i) {
        void* c = data[i];
        const char* nm = (c && Readable(c, 0x18) && Readable(At<const char*>(c, kName), 1)) ? Name(c) : "-";
        Log("    [%d] %p %s", i, c, nm);
    }
}

// Engine world vs parent world * local, under both matrix conventions.
void CheckLayout() {
    double errA = 0, errB = 0, errPos = 0;
    const char* worst = "";
    int checked = 0;
    for (size_t i = 0; i < g_nodes.size(); ++i) {
        void* node = g_nodes[i];
        if (!node) continue;
        // Finger bones are the plugin's own (attached under the hands, see
        // FingerSlot); Skyrim has not updated their world transforms yet.
        const char* nodeName = Name(node);
        if (nodeName && (strstr(nodeName, "Finger") || strstr(nodeName, "Snake Strap"))) continue;
        void* parent = At<void*>(node, kParent);
        if (!parent) continue;
        float* pr = &At<float>(parent, kWorldRot);
        float* lr = &At<float>(node, kLocalRot);
        float* wr = &At<float>(node, kWorldRot);
        float a[9], b[9], pt[9], lt[9];
        MatMul(pr, lr, a);
        for (int r = 0; r < 3; ++r)
            for (int c = 0; c < 3; ++c) pt[r * 3 + c] = pr[c * 3 + r], lt[r * 3 + c] = lr[c * 3 + r];
        float bt[9];
        MatMul(pt, lt, bt);
        for (int r = 0; r < 3; ++r)
            for (int c = 0; c < 3; ++c) b[r * 3 + c] = bt[c * 3 + r];
        double ea = 0, eb = 0;
        for (int k = 0; k < 9; ++k) ea = (std::max)(ea, (double)std::fabs(a[k] - wr[k])), eb = (std::max)(eb, (double)std::fabs(b[k] - wr[k]));
        float* lp = &At<float>(node, kLocalPos);
        float ps = At<float>(parent, kWorldScale), sc[3] = {lp[0] * ps, lp[1] * ps, lp[2] * ps}, rp[3];
        MatVec(pr, sc, rp);
        float* pp = &At<float>(parent, kWorldPos);
        float* wp = &At<float>(node, kWorldPos);
        double ep = 0;
        for (int k = 0; k < 3; ++k) ep = (std::max)(ep, (double)std::fabs(pp[k] + rp[k] - wp[k]));
        if (ea > errA) worst = g_boneNames[i].c_str();
        errA = (std::max)(errA, ea), errB = (std::max)(errB, eb), errPos = (std::max)(errPos, ep);
        ++checked;
    }
    bool ok = checked > 0 && errA < 0.02 && errPos < 0.5;
    if (ok) g_layoutVerified = true;
    g_layoutOk = ok || g_layoutVerified;
    ++g_layoutRetries;
    Log("layout check over %d bones: rot err (P*L) %.4f [worst %s], rot err (transposed) %.4f, pos err %.3f -> %s", checked,
        errA, worst, errB, errPos,
        ok ? "OK, writes allowed"
           : g_layoutVerified ? "mismatch (world transforms not updated yet; layout verified earlier), writes allowed"
                              : "MISMATCH, writes disabled until a later check passes");
}

// ---- firing (Skyrim projectiles) -------------------------------------------------
//
// Snake's controller decides when a shot happens (rate of fire, magazine,
// reload) and what the bullet does (muzzle speed, drop). Each shot becomes a
// real Skyrim projectile launched on the main thread through SKSE's task
// interface, so Skyrim handles hits against the world, damage, kills and NPC
// reactions.
//
// The bullet is a private in-memory copy of vanilla records (Hunting Bow
// 0x13985, Iron Arrow 0x1397D and its projectile): the copies get MGSV's
// muzzle speed and drop per shot, a configurable damage (snake.ini
// BulletDamage), no pick-up, and are never registered with the game, so the
// vanilla bow and arrows are untouched.
//
// Shots fly along the camera's crosshair ray, starting at the depth of
// Snake's right hand.
//
// Addresses: Projectile::Launch, the LaunchData vtable and the PlayerCamera
// singleton from the Address Library database (versionlib-1-7-104-0.bin:
// flat u32 offset table indexed by ID after a 0x60-byte header);
// LookupFormByID from SKSE 2.3.1 (0x1E5860).

constexpr uint32_t kIdProjectileLaunch = 44108;   // Projectile::Launch (AE id)
constexpr uint32_t kIdLaunchDataVtable = 187478;  // VTABLE_Projectile__LaunchData (AE id)
constexpr uint32_t kIdPlayerCamera = 400802;      // PlayerCamera* singleton (AE id)
constexpr uint32_t kIdAttackBlockVtable = 208719; // VTABLE_AttackBlockHandler (AE id)
constexpr uint32_t kIdModelDemand = 75782;        // BSModelDB::Demand (AE id)
constexpr uint32_t kIdStringPoolGetEntry = 69161;  // BSStringPool::GetEntry<char> (AE id)
constexpr uint32_t kIdPlayerControls = 400864;     // PlayerControls singleton (AE id; checked before use)
constexpr uint32_t kIdLookupRefByHandle = 12332;  // LookupReferenceByHandle (AE id; diagnostics, checked before use)
constexpr size_t kModelPath = 0x48;                // BGSProjectile TESModel (+0x40) model path (BSFixedString)
constexpr const char* kBulletModel = "snake\\bullet.nif";  // empty model written by mgsv skyrim-export
constexpr uintptr_t kLookupFormByID = 0x001E5860;
constexpr uint32_t kHuntingBow = 0x00013985, kIronArrow = 0x0001397D;
constexpr size_t kFormType = 0x1A;
constexpr size_t kAmmoProjectile = 0x110;  // TESAmmo AMMO_DATA.projectile
constexpr size_t kAmmoDamage = 0x11C;      // TESAmmo AMMO_DATA.damage (float)
constexpr size_t kWeapDamage = 0xC8;       // TESObjectWEAP TESAttackDamageForm.attackDamage (u16)
constexpr size_t kProjData = 0x80;         // BGSProjectile BGSProjectileData
constexpr size_t kSizeProjectile = 0x138, kSizeWeapon = 0x220, kSizeAmmo = 0x128;
constexpr size_t kCameraRoot = 0x20;       // TESCamera::cameraRoot (NiNode*)
constexpr uint8_t kTypeWeapon = 0x29, kTypeAmmo = 0x2A, kTypeProjectile = 0x32, kTypeCell = 0x3C;
constexpr uint8_t kTypeAlchemy = 0x2E;
constexpr uint32_t kParalysisPoison = 0x00074A3B;  // Skyrim.esm ALCH Paralyze05

struct LaunchData {
    void* vtable;
    float origin[3];         // 08
    float contactNormal[3];  // 14
    void* projectileBase;    // 20
    void* shooter;           // 28
    void* combatController;  // 30
    void* weaponSource;      // 38
    void* ammoSource;        // 40
    float angleZ;            // 48
    float angleX;            // 4C
    void* unk50;             // 50
    void* desiredTarget;     // 58
    float unk60, unk64;      // 60
    void* parentCell;        // 68
    void* spell;             // 70
    uint32_t castingSource;  // 78
    uint32_t pad7C;
    void* enchantItem;       // 80
    void* poison;            // 88
    int32_t area;            // 90
    float power;             // 94
    float scale;             // 98
    bool alwaysHit, noDamageOutsideCombat, autoAim, chainShatter;  // 9C
    bool useOrigin, deferInitialization, forceConeOfFire;          // A0
    uint8_t padA3[5];
};
static_assert(sizeof(LaunchData) == 0xA8, "LaunchData layout");

// BGSProjectileData (DATA), the part the bullet changes.
struct ProjectileData {
    uint16_t flags;  // 00
    uint16_t types;  // 02
    float gravity;   // 04 (multiple of world gravity)
    float speed;     // 08 (units/s)
    float range;     // 0C (units)
};
constexpr uint16_t kProjCanPickUp = 1 << 6, kProjSupersonic = 1 << 7, kProjNoAimCorrection = 1 << 10;
constexpr uint16_t kProjTypeArrow = 1 << 6;
constexpr float kArrowSpeedScale = 0.148f;  // launched speed / record speed (measured)

using LaunchFn = uint32_t* (*)(uint32_t* handle, LaunchData* data);
using LookupFn = void* (*)(uint32_t formId);

struct Firing {
    LaunchFn launch = nullptr;
    void* launchDataVtable = nullptr;
    void** camera = nullptr;  // &PlayerCamera singleton
    void** attackBlockVtable = nullptr;
    void* modelDemand = nullptr;
    void* getEntry = nullptr;  // BSStringPool::GetEntry
    void** playerControls = nullptr;  // &PlayerControls singleton
    void* weapon = nullptr;   // bullet copies
    void* ammo = nullptr;
    void* projectile = nullptr;
    void* tranq = nullptr;    // paralysis poison carried by sleep darts
    int damage = 48;          // Skyrim damage of 600 MGSV lethal damage (an MRS-4 round)
    size_t cellOffset = 0;
    bool ready = false;
} g_fire;

struct PendingShot {
    float speed, gravity;  // m/s, m/s^2
    float lethal, sleep;   // MGSV damage per hit
};

SKSETaskInterface* g_tasks = nullptr;

// Diagnostics (control file trackshot=1): one bullet followed through its
// flight, logging how far along the aim ray it got and how far below it.
using LookupRefFn = bool (*)(const uint32_t& handle, void*& out);
LookupRefFn g_lookupRef = nullptr;
std::atomic<int> g_trackShot{0};
struct TrackedShot {
    uint32_t handle = 0;
    float origin[3] = {}, dir[3] = {};
    std::chrono::steady_clock::time_point start;
    float along = 0, drop = 0, t = 0;
    int samples = 0;
    std::atomic<bool> active{false};
} g_tracked;
std::mutex g_shotMutex;
std::vector<PendingShot> g_pendingShots;
std::atomic<int> g_shotsFired{0};

uint8_t FormType(void* form) { return form && Readable(form, 0x20) ? At<uint8_t>(form, kFormType) : 0; }

bool LoadAddressLibrary() {
    std::wstring path = PluginDir() + L"\\versionlib-1-7-104-0.bin";
    FILE* f = _wfsopen(path.c_str(), L"rb", _SH_DENYNO);
    if (!f) {
        Log("firing: Address Library database not found (%s)", Narrow(path).c_str());
        return false;
    }
    uint32_t header[0x18] = {};
    fread(header, 4, 0x18, f);
    if (header[0] != 5 || header[1] != 1 || header[2] != 7 || header[3] != 104) {
        Log("firing: unexpected Address Library header (format %u, version %u.%u.%u)", header[0], header[1], header[2], header[3]);
        fclose(f);
        return false;
    }
    uint32_t count = header[0x17];
    auto read = [&](uint32_t id) -> uintptr_t {
        if (id >= count) return 0;
        uint32_t off = 0;
        _fseeki64(f, 0x60 + int64_t(id) * 4, SEEK_SET);
        fread(&off, 4, 1, f);
        return off;
    };
    uintptr_t base = reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr));
    uintptr_t launch = read(kIdProjectileLaunch), vt = read(kIdLaunchDataVtable), cam = read(kIdPlayerCamera), ab = read(kIdAttackBlockVtable),
              demand = read(kIdModelDemand), entry = read(kIdStringPoolGetEntry), controls = read(kIdPlayerControls),
              lookupRef = read(kIdLookupRefByHandle);
    fclose(f);
    if (!launch || !vt) {
        Log("firing: ids missing from the database");
        return false;
    }
    g_fire.launch = reinterpret_cast<LaunchFn>(base + launch);
    g_fire.launchDataVtable = reinterpret_cast<void*>(base + vt);
    g_fire.camera = cam ? reinterpret_cast<void**>(base + cam) : nullptr;
    g_fire.attackBlockVtable = ab ? reinterpret_cast<void**>(base + ab) : nullptr;
    g_fire.modelDemand = demand ? reinterpret_cast<void*>(base + demand) : nullptr;
    g_fire.getEntry = entry ? reinterpret_cast<void*>(base + entry) : nullptr;
    g_fire.playerControls = controls ? reinterpret_cast<void**>(base + controls) : nullptr;
    g_lookupRef = lookupRef ? reinterpret_cast<LookupRefFn>(base + lookupRef) : nullptr;
    Log("firing: %u ids; Projectile::Launch +%llX, LaunchData vtable +%llX, PlayerCamera +%llX", count,
        (unsigned long long)launch, (unsigned long long)vt, (unsigned long long)cam);
    return true;
}

// A private copy of a form: same vtable and data, never registered with the
// game (no form ID lookup finds it, nothing frees it).
void* CopyForm(void* src, size_t size) {
    if (!Readable(src, size)) return nullptr;
    void* p = _aligned_malloc(size, 16);
    memcpy(p, src, size);
    return p;
}

int IniInt(const wchar_t* key, int fallback) {
    std::wstring ini = PluginDir() + L"\\snake.ini";
    return GetPrivateProfileIntW(L"Snake", key, fallback, ini.c_str());
}

// A pooled string (BSFixedString data) for `text`, or null. GetEntry writes
// the string's data pointer; it is checked against `known`, a string the pool
// already holds, which must come back as the very same pointer.
const char* SafePooled(const char* text, const char* known) {
    using Fn = void (*)(const char**, const char*, bool);
    auto get = reinterpret_cast<Fn>(g_fire.getEntry);
    if (!get || !known) return nullptr;
    __try {
        const char* again = nullptr;
        get(&again, known, false);
        if (again != known) return nullptr;
        const char* out = nullptr;
        get(&out, text, false);
        return out && _stricmp(out, text) == 0 ? out : nullptr;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return nullptr;
    }
}

// Point the bullet projectile at the empty bullet model (bullets fly
// unseen and leave no arrow stuck in what they hit).
void SetBulletModel() {
    const char* arrowModel = At<const char*>(g_fire.projectile, kModelPath);
    bool plausible = arrowModel && Readable(arrowModel, 4) && strstr(arrowModel, ".nif");
    Log("firing: arrow projectile model %s", plausible ? arrowModel : "(not a model path; layout differs)");
    std::wstring file = PluginDir() + L"\\..\\..\\meshes\\snake\\bullet.nif";
    if (!plausible || GetFileAttributesW(file.c_str()) == INVALID_FILE_ATTRIBUTES) {
        Log("firing: bullets keep the arrow model (no %s)", Narrow(file).c_str());
        return;
    }
    const char* pooled = SafePooled(kBulletModel, arrowModel);
    if (!pooled) {
        Log("firing: string pool check failed; bullets keep the arrow model");
        return;
    }
    At<const char*>(g_fire.projectile, kModelPath) = pooled;
    Log("firing: bullet model %s", pooled);
}

// A float game / INI setting found by name (see FindFloatSetting).
struct FloatSetting {
    float* value = nullptr;
    float original = 0;
};
bool SafeFindFloatSetting(const char* name, FloatSetting& out);

bool SetUpFiring() {
    auto lookup = reinterpret_cast<LookupFn>(reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr)) + kLookupFormByID);
    void* bow = lookup(kHuntingBow);
    void* arrow = lookup(kIronArrow);
    void* proj = arrow && Readable(arrow, kAmmoProjectile + 8) ? At<void*>(arrow, kAmmoProjectile) : nullptr;
    Log("firing: bow %p (type %02X), arrow %p (type %02X), projectile %p (type %02X)", bow, FormType(bow), arrow,
        FormType(arrow), proj, FormType(proj));
    if (FormType(bow) != kTypeWeapon || FormType(arrow) != kTypeAmmo || FormType(proj) != kTypeProjectile) {
        Log("firing: form types do not match; firing disabled");
        return false;
    }
    auto& pd = At<ProjectileData>(proj, kProjData);
    uint16_t bowDamage = At<uint16_t>(bow, kWeapDamage);
    float arrowDamage = At<float>(arrow, kAmmoDamage);
    Log("firing: vanilla bow damage %u, arrow damage %.1f; projectile flags %04X types %04X gravity %.2f speed %.0f range %.0f",
        bowDamage, arrowDamage, pd.flags, pd.types, pd.gravity, pd.speed, pd.range);
    // Expected vanilla values (Hunting Bow 7, Iron Arrow 8, arrow type):
    // anything else means the layout differs on this runtime.
    if (bowDamage != 7 || std::fabs(arrowDamage - 8.0f) > 0.01f || pd.types != kProjTypeArrow) {
        Log("firing: unexpected record layout; firing disabled");
        return false;
    }
    g_fire.projectile = CopyForm(proj, kSizeProjectile);
    g_fire.weapon = CopyForm(bow, kSizeWeapon);
    g_fire.ammo = CopyForm(arrow, kSizeAmmo);
    if (!g_fire.projectile || !g_fire.weapon || !g_fire.ammo) {
        Log("firing: could not copy the records");
        return false;
    }
    auto& bullet = At<ProjectileData>(g_fire.projectile, kProjData);
    bullet.flags = (bullet.flags & ~kProjCanPickUp) | kProjSupersonic | kProjNoAimCorrection;
    bullet.range = 100000.0f;  // ~1.4 km
    // BulletDamage is the MRS-4's damage; other weapons scale by their MGSV
    // lethal damage (MRS-4: 600). Tranquilizer rounds (no lethal damage,
    // sleep damage instead, e.g. the WU S.Pistol) do no damage and carry
    // Skyrim's strongest paralysis poison (Paralyze05), which drops the
    // target like MGSV's sleep darts.
    int damage = IniInt(L"BulletDamage", 48);
    g_fire.damage = std::clamp(damage, 0, 65535);
    At<uint16_t>(g_fire.weapon, kWeapDamage) = static_cast<uint16_t>(g_fire.damage);
    void* tranq = lookup(kParalysisPoison);
    g_fire.tranq = FormType(tranq) == kTypeAlchemy ? tranq : nullptr;
    Log("firing: paralysis poison %p (type %02X)%s", tranq, FormType(tranq), g_fire.tranq ? "" : " -- sleep darts unavailable");
    At<float>(g_fire.ammo, kAmmoDamage) = 0.0f;
    At<void*>(g_fire.ammo, kAmmoProjectile) = g_fire.projectile;
    SetBulletModel();
    for (const char* name : {"fArrowMaxDistance", "fProjectileMaxDistance", "fArrowOptimalDistance", "fArrowSpeedMult",
                             "fArrowGravityBase", "fArrowGravityMin", "fArrowGravityMult", "fArrowWeakGravity",
                             "fArrowMinPower", "fArrowMinVelocity", "fArrowMinBowVelocity", "fProjectileDefaultTracerRange"}) {
        FloatSetting f;
        if (SafeFindFloatSetting(name, f)) Log("firing: game setting %s = %.3f", name, *f.value);
        else Log("firing: game setting %s not found", name);
    }
    Log("firing: bullet records ready (damage %d, no pick-up)", damage);
    return true;
}

// The player's parent cell pointer: SE keeps it at +0x60; this runtime has
// shifted reference data by 8 bytes, so check both and keep the one that
// points at a cell.
void* PlayerCell(void* player) {
    if (!g_fire.cellOffset) {
        for (size_t off : {size_t(0x68), size_t(0x60)}) {
            void* c = At<void*>(player, off);
            Log("firing: parent cell candidate +%zX -> %p (type %02X)", off, c, FormType(c));
            if (FormType(c) == kTypeCell) {
                g_fire.cellOffset = off;
                break;
            }
        }
        if (!g_fire.cellOffset) return nullptr;
    }
    return At<void*>(player, g_fire.cellOffset);
}

// The camera's position and view direction (unit), from the camera root's
// world transform: Bethesda's forward axis is local +Y.
bool CameraAim(float* pos, float* dir) {
    if (!g_fire.camera) return false;
    void* cam = *g_fire.camera;
    if (!cam || !Readable(cam, kCameraRoot + 8)) return false;
    void* root = At<void*>(cam, kCameraRoot);
    if (!root || !Readable(root, kWorldScale + 4)) return false;
    const float* m = &At<float>(root, kWorldRot);
    memcpy(pos, &At<float>(root, kWorldPos), 3 * sizeof(float));
    float d[3] = {m[1], m[4], m[7]};
    float len = std::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
    if (!(len > 0.5f && len < 2.0f)) return false;
    for (int k = 0; k < 3; ++k) dir[k] = d[k] / len;
    return true;
}

// Diagnostics: the camera singleton's pointer fields that look like scene
// nodes, with their world position and axes, next to the player's.
void DumpCamera(void* player) {
    __try {
        const float* a = &At<float>(player, kRefAngle);
        const float* p = &At<float>(player, kRefPosition);
        Log("camera dump: player pos %.0f %.0f %.0f, pitch %.3f heading %.3f", p[0], p[1], p[2], a[0], a[2]);
        void* cam = g_fire.camera ? *g_fire.camera : nullptr;
        if (!cam || !Readable(cam, 0x200)) return;
        for (size_t off = 0x08; off < 0x200; off += 8) {
            void* q = At<void*>(cam, off);
            if (!q || !Readable(q, kWorldScale + 4)) continue;
            void* nameP = At<void*>(q, kName);
            const char* name = nameP && Readable(nameP, 4) ? static_cast<const char*>(nameP) : nullptr;
            if (!name || !Readable(name, 32)) continue;
            const float* m = &At<float>(q, kWorldRot);
            const float* w = &At<float>(q, kWorldPos);
            Log("  cam+%03zX %p '%.31s' pos %.0f %.0f %.0f  X %.2f %.2f %.2f  Y %.2f %.2f %.2f  rowY %.2f %.2f %.2f", off, q, name,
                w[0], w[1], w[2], m[0], m[3], m[6], m[1], m[4], m[7], m[3], m[4], m[5]);
        }
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        Log("camera dump faulted");
    }
}

bool LaunchOne(void* player, const PendingShot& shot) {
    __try {
        void* cell = PlayerCell(player);
        if (!cell) return false;
        // Aim: the crosshair ray, else the actor's facing.
        const float* angle = &At<float>(player, kRefAngle);  // pitch, roll, heading
        float* pos = &At<float>(player, kRefPosition);
        float cam[3] = {pos[0], pos[1], pos[2] + 110.0f};
        float dir[3] = {std::sin(angle[2]) * std::cos(angle[0]), std::cos(angle[2]) * std::cos(angle[0]), -std::sin(angle[0])};
        CameraAim(cam, dir);
        // Start on that ray at the depth of the right hand, so the shot goes
        // where the crosshair points and leaves from beside the gun.
        float hand[3] = {pos[0], pos[1], pos[2] + 110.0f};
        for (size_t i = 0; i < g_nodes.size() && i < g_boneNames.size(); ++i) {
            if (g_boneNames[i] == "NPC R Hand [RHnd]" && g_nodes[i]) memcpy(hand, &At<float>(g_nodes[i], kWorldPos), sizeof hand);
        }
        float depth = (hand[0] - cam[0]) * dir[0] + (hand[1] - cam[1]) * dir[1] + (hand[2] - cam[2]) * dir[2];
        depth = (std::max)(depth, 0.0f) + 30.0f;

        auto& bullet = At<ProjectileData>(g_fire.projectile, kProjData);
        // m/s -> units/s. Skyrim launches arrow projectiles at a fraction of
        // the record's speed (measured with trackshot=1: 0.148 of it, at
        // 450 and at 200 m/s), so the record asks for that much more: at the
        // record's speed the MRS-4's rounds flew at 67 m/s and fell 4 m in
        // 45 m.
        bullet.speed = shot.speed * 70.0f / kArrowSpeedScale;
        bullet.gravity = shot.gravity / 9.81f;  // m/s^2 -> multiple of g
        float scaled = g_fire.damage * shot.lethal / 600.0f;
        At<uint16_t>(g_fire.weapon, kWeapDamage) = static_cast<uint16_t>(std::clamp(scaled + 0.5f, 0.0f, 65535.0f));

        LaunchData d{};
        d.vtable = g_fire.launchDataVtable;
        for (int k = 0; k < 3; ++k) d.origin[k] = cam[k] + dir[k] * depth;
        d.projectileBase = g_fire.projectile;
        d.shooter = player;
        d.weaponSource = g_fire.weapon;
        d.ammoSource = g_fire.ammo;
        if (shot.sleep > 0 && shot.lethal <= 0) d.poison = g_fire.tranq;
        d.angleZ = std::atan2(dir[0], dir[1]);
        d.angleX = -std::asin((std::max)(-1.0f, (std::min)(1.0f, dir[2])));
        d.parentCell = cell;
        d.castingSource = 2;  // kOther
        d.power = 1.0f;
        d.scale = 1.0f;
        d.useOrigin = true;
        d.autoAim = false;
        uint32_t handle = 0;
        g_fire.launch(&handle, &d);
        if (g_trackShot && g_lookupRef && handle && !g_tracked.active) {
            g_tracked.handle = handle;
            memcpy(g_tracked.origin, d.origin, sizeof d.origin);
            memcpy(g_tracked.dir, dir, sizeof dir);
            g_tracked.start = std::chrono::steady_clock::now();
            g_tracked.along = g_tracked.drop = g_tracked.t = 0;
            g_tracked.samples = 0;
            g_tracked.active = true;
        }
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

class ShotTask : public TaskDelegate {
public:
    void Run() override {
        std::vector<PendingShot> shots;
        {
            std::lock_guard<std::mutex> lock(g_shotMutex);
            shots.swap(g_pendingShots);
        }
        void* player = ThePlayer();
        for (const auto& s : shots) {
            if (!player || !g_fire.ready) break;
            if (!LaunchOne(player, s)) {
                Log("firing: launch faulted; firing disabled");
                g_fire.ready = false;
                break;
            }
            int total = ++g_shotsFired;
            if (total <= 3 || total % 30 == 0) {
                float cam[3], dir[3];
                bool ok = CameraAim(cam, dir);
                Log("firing: shot %d launched (%.0f m/s, drop %.1f m/s2; camera %s dir %.2f %.2f %.2f)", total, s.speed,
                    s.gravity, ok ? "ok" : "unavailable", ok ? dir[0] : 0, ok ? dir[1] : 0, ok ? dir[2] : 0);
            }
        }
    }
    void Dispose() override { delete this; }
};

// The tracked bullet's position, or false once it is gone. The reference
// comes back with a reference taken (NiPointer), given back at once; it is
// only used if its base object is the bullet projectile record.
bool TrackedPosition(float* pos, bool* isBullet) {
    __try {
        void* ref = nullptr;
        if (!g_lookupRef(g_tracked.handle, ref) || !ref) return false;
        *isBullet = Readable(ref, 0x60) && At<void*>(ref, 0x40) == g_fire.projectile;
        if (*isBullet) memcpy(pos, &At<float>(ref, kRefPosition), 3 * sizeof(float));
        InterlockedDecrement(reinterpret_cast<volatile LONG*>(reinterpret_cast<char*>(ref) + 8));
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

class TrackTask : public TaskDelegate {
public:
    void Run() override {
        if (!g_tracked.active) return;
        float t = std::chrono::duration<float>(std::chrono::steady_clock::now() - g_tracked.start).count();
        float pos[3];
        bool isBullet = false;
        if (!TrackedPosition(pos, &isBullet) || !isBullet || t > 10.0f) {
            Log("track: bullet %s after %.3f s: last seen %.1f m along the aim, %.2f m below it (%d samples)",
                isBullet || t > 10.0f ? "still flying" : "gone", t, g_tracked.along, g_tracked.drop, g_tracked.samples);
            g_tracked.active = false;
            return;
        }
        float rel[3] = {pos[0] - g_tracked.origin[0], pos[1] - g_tracked.origin[1], pos[2] - g_tracked.origin[2]};
        float along = rel[0] * g_tracked.dir[0] + rel[1] * g_tracked.dir[1] + rel[2] * g_tracked.dir[2];
        float lineZ = g_tracked.origin[2] + g_tracked.dir[2] * along;
        g_tracked.along = along / 70.0f;
        g_tracked.drop = (lineZ - pos[2]) / 70.0f;
        if (g_tracked.samples++ % 3 == 0) Log("track: %.3f s: %.1f m along, %.2f m below the aim", t, g_tracked.along, g_tracked.drop);
    }
    void Dispose() override { delete this; }
};

void QueueShots(const SnakeShot* shots, uint32_t n) {
    if (!n || !g_fire.ready || !g_tasks) return;
    bool first;
    {
        std::lock_guard<std::mutex> lock(g_shotMutex);
        first = g_pendingShots.empty();
        for (uint32_t i = 0; i < n; ++i) g_pendingShots.push_back(
                {shots[i].speed, shots[i].gravity, shots[i].lethal_damage, shots[i].sleep_damage});
    }
    if (first) g_tasks->AddTask(new ShotTask());
}

// While Snake aims, the actor has its (empty) hands drawn, as Skyrim does
// for a drawn bow: the actor turns with the camera and moves in combat
// strafes, so Snake's gun points at the crosshair. Snake's pose overrides the
// draw/sheathe animations. Actor::DrawWeaponMagicHands is vtable slot 0xA6;
// the weapon state is actorState2 (actor + 0xCC) bits 5-7.
constexpr size_t kVtDrawWeaponMagicHands = 0xA6;
constexpr size_t kActorState2 = 0xCC;
enum WeaponState { kSheathed = 0, kWantToDraw, kDrawing, kDrawn, kWantToSheathe, kSheathing };

int WeaponStateOf(void* actor) { return (At<uint32_t>(actor, kActorState2) >> 5) & 7; }

bool DrawHands(void* player, bool draw) {
    __try {
        using Fn = void (*)(void*, bool);
        reinterpret_cast<Fn>(Vtable(player)[kVtDrawWeaponMagicHands])(player, draw);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

class DrawTask : public TaskDelegate {
public:
    explicit DrawTask(bool draw) : draw_(draw) {}
    void Run() override {
        void* player = ThePlayer();
        if (!player) return;
        int state = WeaponStateOf(player);
        bool drawn = state >= kWantToDraw && state <= kDrawn;
        if (drawn == draw_) return;
        if (!DrawHands(player, draw_)) Log("draw hands faulted");
    }
    void Dispose() override { delete this; }

private:
    bool draw_;
};

void RequestHands(bool draw) {
    if (g_tasks) g_tasks->AddTask(new DrawTask(draw));
}

// ---- gunshot sounds --------------------------------------------------------------
//
// Each weapon's shot is a .wav exported from the user's MGSV sound banks by
// `mgsv skyrim-export` (Data/SKSE/Plugins/snake_sounds/<weapon id>_shot.wav).
// The sound is MGSV's own shot event for the weapon, tail included (the
// MRS-4's rings out for 2.6 s). Played through XAudio2 with a pool of voices
// per sound, sized so full-auto shots overlap for their whole length instead
// of cutting each other off (the oldest, quietest tail is reused first).
// Volume: snake.ini ShotVolume (percent, default 70).

struct ShotSound {
    std::vector<uint8_t> data;  // PCM samples
    WAVEFORMATEX format{};
    std::vector<IXAudio2SourceVoice*> voices;
    size_t next = 0;
};

IXAudio2* g_audio = nullptr;
IXAudio2MasteringVoice* g_master = nullptr;
std::vector<std::vector<ShotSound>> g_shotSounds;  // by weapon index: its variations

bool ReadWav(const std::wstring& path, ShotSound& out) {
    FILE* f = _wfsopen(path.c_str(), L"rb", _SH_DENYNO);
    if (!f) return false;
    std::vector<uint8_t> b;
    uint8_t chunk[65536];
    size_t n;
    while ((n = fread(chunk, 1, sizeof chunk, f)) > 0) b.insert(b.end(), chunk, chunk + n);
    fclose(f);
    if (b.size() < 12 || memcmp(b.data(), "RIFF", 4) != 0 || memcmp(b.data() + 8, "WAVE", 4) != 0) return false;
    bool haveFmt = false;
    for (size_t p = 12; p + 8 <= b.size();) {
        uint32_t len = *reinterpret_cast<uint32_t*>(&b[p + 4]);
        if (p + 8 + len > b.size()) break;
        if (memcmp(&b[p], "fmt ", 4) == 0 && len >= 16) {
            memcpy(&out.format, &b[p + 8], 16);
            out.format.cbSize = 0;
            haveFmt = out.format.wFormatTag == WAVE_FORMAT_PCM;
        } else if (memcmp(&b[p], "data", 4) == 0) {
            out.data.assign(b.begin() + p + 8, b.begin() + p + 8 + len);
        }
        p += 8 + len + (len & 1);
    }
    return haveFmt && !out.data.empty();
}

void SetUpFoley();

void SetUpSounds() {
    if (FAILED(XAudio2Create(&g_audio, 0, XAUDIO2_DEFAULT_PROCESSOR)) || !g_audio) {
        Log("sound: XAudio2 unavailable");
        return;
    }
    if (FAILED(g_audio->CreateMasteringVoice(&g_master))) {
        Log("sound: no mastering voice");
        return;
    }
    std::wstring ini = PluginDir() + L"\\snake.ini";
    float volume = GetPrivateProfileIntW(L"Snake", L"ShotVolume", 70, ini.c_str()) / 100.0f;
    g_master->SetVolume(volume);
    uint32_t weapons = g_snake.weapon_count(g_runtime);
    g_shotSounds.resize(weapons);
    for (uint32_t i = 0; i < weapons; ++i) {
        // "snake\wp_west_ar_010.nif" -> "wp_west_ar_010"
        std::string mesh = g_snake.weapon_mesh(g_runtime, i) ? g_snake.weapon_mesh(g_runtime, i) : "";
        size_t slash = mesh.find_last_of("\\/"), dot = mesh.rfind('.');
        std::string id = mesh.substr(slash + 1, dot - slash - 1);
        // Variations: <id>_shot.wav, <id>_shot1.wav, <id>_shot2.wav, ...
        std::wstring base = PluginDir() + L"\\snake_sounds\\" + std::wstring(id.begin(), id.end()) + L"_shot";
        for (int k = 0; k < 16; ++k) {
            std::wstring path = base + (k ? std::to_wstring(k) : L"") + L".wav";
            ShotSound s;
            if (!ReadWav(path, s)) break;
            // Enough voices for 14 shots a second (840 rpm) to ring out.
            double seconds = s.data.size() / double(s.format.nAvgBytesPerSec);
            int pool = std::clamp(static_cast<int>(std::ceil(seconds * 14.0)), 4, 48);
            for (int v = 0; v < pool; ++v) {
                IXAudio2SourceVoice* voice = nullptr;
                if (SUCCEEDED(g_audio->CreateSourceVoice(&voice, &s.format))) s.voices.push_back(voice);
            }
            Log("sound: %s shot %d: %u Hz x%u, %.2f s, %zu voices", id.c_str(), k, s.format.nSamplesPerSec,
                s.format.nChannels, seconds, s.voices.size());
            g_shotSounds[i].push_back(std::move(s));
        }
        if (g_shotSounds[i].empty()) Log("sound: no shot sound for %s (%s.wav)", id.c_str(), Narrow(base).c_str());
    }
    Log("sound: volume %.2f", volume);
    SetUpFoley();
}

// Muzzle flash: the exported gun mesh has four flash shapes
// ("muzzle_flash:0".."3", one per flipbook cell); one random cell shows for
// a few frames after each shot.
std::atomic<int> g_flashCell{-1};
std::atomic<int64_t> g_flashUntil{0};  // steady_clock milliseconds

int64_t NowMs() {
    return std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now().time_since_epoch())
        .count();
}

void TriggerFlash() {
    static uint32_t seed = 12345;
    seed = seed * 1664525u + 1013904223u;
    g_flashCell = static_cast<int>((seed >> 16) % 4);
    g_flashUntil = NowMs() + 45;
}

void PlayShot(int weapon) {
    if (weapon < 0 || weapon >= (int)g_shotSounds.size() || g_shotSounds[weapon].empty()) return;
    // A random variation, never the same one twice in a row.
    auto& variations = g_shotSounds[weapon];
    static uint32_t seed = 0x9E3779B9u;
    static size_t last = SIZE_MAX;
    seed = seed * 1664525u + 1013904223u;
    size_t pick = (seed >> 16) % variations.size();
    if (pick == last && variations.size() > 1) pick = (pick + 1) % variations.size();
    last = pick;
    ShotSound& s = variations[pick];
    if (s.voices.empty()) return;
    IXAudio2SourceVoice* v = s.voices[s.next++ % s.voices.size()];
    v->Stop();
    v->FlushSourceBuffers();
    XAUDIO2_BUFFER buf{};
    buf.Flags = XAUDIO2_END_OF_STREAM;
    buf.AudioBytes = static_cast<UINT32>(s.data.size());
    buf.pAudioData = s.data.data();
    if (SUCCEEDED(v->SubmitSourceBuffer(&buf))) v->Start();
}

// ---- foley ------------------------------------------------------------------------
//
// Snake's footsteps, hands on the ground, suit and gear rattle, weapon
// rattle and the chest landing on a dive: rendered by snake.dll from MGSV's
// own sound bank with MGSV's switches (surface, action), 48 kHz stereo, and
// played through a pool of voices here (each keeps its copy of the samples
// while it plays). The surface is MGSV's footstep symbol for the ground:
// stone (ROCKA) in interiors, soil (SOILA) outdoors. Volume: snake.ini
// FoleyVolume (percent, default 100) for all of it, FootstepVolume (percent
// of that, default 100) for the feet and the hands on the ground, and
// RattleVolume (default 10) for the suit and weapon rattle, which played far
// louder than the rest of the game.

struct FoleyVoice {
    IXAudio2SourceVoice* voice = nullptr;
    std::vector<int16_t> samples;
};
std::vector<FoleyVoice> g_foleyVoices;
size_t g_foleyNext = 0;
float g_foleyLevel = 1.0f;     // voice volume for foley (FoleyVolume over the master)
float g_footstepLevel = 1.0f;  // FootstepVolume
float g_rattleLevel = 0.10f;   // RattleVolume

void SetUpFoley() {
    if (!g_audio || !g_snake.player_foley) return;
    WAVEFORMATEX f{};
    f.wFormatTag = WAVE_FORMAT_PCM;
    f.nChannels = 2;
    f.nSamplesPerSec = 48000;
    f.wBitsPerSample = 16;
    f.nBlockAlign = 4;
    f.nAvgBytesPerSec = 48000 * 4;
    std::wstring ini = PluginDir() + L"\\snake.ini";
    float master = GetPrivateProfileIntW(L"Snake", L"ShotVolume", 70, ini.c_str()) / 100.0f;
    float volume = GetPrivateProfileIntW(L"Snake", L"FoleyVolume", 100, ini.c_str()) / 100.0f;
    g_footstepLevel = GetPrivateProfileIntW(L"Snake", L"FootstepVolume", 100, ini.c_str()) / 100.0f;
    g_rattleLevel = GetPrivateProfileIntW(L"Snake", L"RattleVolume", 10, ini.c_str()) / 100.0f;
    // The mastering voice carries ShotVolume; foley has its own level.
    g_foleyLevel = master > 0.01f ? volume / master : volume;
    for (int k = 0; k < 24; ++k) {
        FoleyVoice v;
        if (FAILED(g_audio->CreateSourceVoice(&v.voice, &f))) break;
        g_foleyVoices.push_back(std::move(v));
    }
    Log("foley: %zu voices, volume %.2f, footsteps %.2f, rattle %.2f", g_foleyVoices.size(), volume, g_footstepLevel,
        g_rattleLevel);
}

void PlayFoley(const SnakeSound& sound) {
    if (g_foleyVoices.empty() || !sound.samples || !sound.frames) return;
    // A free voice, else the oldest one.
    FoleyVoice* pick = nullptr;
    for (size_t k = 0; k < g_foleyVoices.size() && !pick; ++k) {
        FoleyVoice& v = g_foleyVoices[(g_foleyNext + k) % g_foleyVoices.size()];
        XAUDIO2_VOICE_STATE st{};
        v.voice->GetState(&st, XAUDIO2_VOICE_NOSAMPLESPLAYED);
        if (st.BuffersQueued == 0) pick = &v;
    }
    if (!pick) pick = &g_foleyVoices[g_foleyNext % g_foleyVoices.size()];
    g_foleyNext = (pick - g_foleyVoices.data() + 1) % g_foleyVoices.size();
    pick->voice->Stop();
    pick->voice->FlushSourceBuffers();
    // SnakeSound.kind: 0-3 feet and hands on the ground, 4-5 suit and weapon
    // rattle.
    float level = sound.kind >= 0 && sound.kind <= 3 ? g_footstepLevel
                  : sound.kind == 4 || sound.kind == 5  ? g_rattleLevel
                                                        : 1.0f;
    pick->voice->SetVolume(g_foleyLevel * level);
    pick->samples.assign(sound.samples, sound.samples + size_t(sound.frames) * 2);
    XAUDIO2_BUFFER buf{};
    buf.Flags = XAUDIO2_END_OF_STREAM;
    buf.AudioBytes = static_cast<UINT32>(pick->samples.size() * sizeof(int16_t));
    buf.pAudioData = reinterpret_cast<const BYTE*>(pick->samples.data());
    if (SUCCEEDED(pick->voice->SubmitSourceBuffer(&buf))) pick->voice->Start();
}

// MGSV's footstep surface for where the player is: stone floors inside,
// soil outside (TESObjectCELL flags +0x40, bit 0 interior).
const char* GroundSurface(void* player) {
    void* cell = player ? PlayerCell(player) : nullptr;
    if (!cell || !Readable(cell, 0x48)) return "SOILA";
    return (At<uint16_t>(cell, 0x40) & 1) ? "ROCKA" : "SOILA";
}

// ---- movement speed ---------------------------------------------------------------
//
// Skyrim's character controller still moves the player (collision, slopes,
// stairs), but at Snake's speed: Snake's controller runs MGSV's locomotion
// internally, and the player's SpeedMult actor value is steered until
// Skyrim's measured ground speed matches Snake's. So walk, run, dash,
// crouch-walk and the prone crawl move at MGSV speeds and his feet do not
// slide. ActorValueOwner is at actor + 0xB8 on this runtime (8 bytes after
// SE, like ActorState); slots 1 GetActorValue, 4 SetBaseActorValue,
// 5 ModActorValue. SpeedMult = 30, CarryWeight = 32 (nudging CarryWeight
// makes Skyrim pick up the new speed).

constexpr size_t kActorValueOwner = 0xB8;
constexpr uint32_t kAvSpeedMult = 30, kAvCarryWeight = 32;

std::atomic<float> g_speedMult{-1.0f};    // current SpeedMult we set (-1: not read yet)
std::atomic<float> g_baseSpeedMult{-1.0f};  // the player's own, restored when Snake mode ends
std::atomic<float> g_wantSpeedMult{-1.0f};
std::atomic<bool> g_speedQueued{false};
std::atomic<bool> g_speedBroken{false};

float SafeGetAv(void* actor, uint32_t av) {
    __try {
        void* owner = reinterpret_cast<char*>(actor) + kActorValueOwner;
        using Fn = float (*)(void*, uint32_t);
        return reinterpret_cast<Fn>(Vtable(owner)[1])(owner, av);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return -1.0f;
    }
}

bool SafeSetSpeedMult(void* actor, float value) {
    __try {
        void* owner = reinterpret_cast<char*>(actor) + kActorValueOwner;
        using SetFn = void (*)(void*, uint32_t, float);
        void** vt = Vtable(owner);
        reinterpret_cast<SetFn>(vt[4])(owner, kAvSpeedMult, value);
        reinterpret_cast<SetFn>(vt[5])(owner, kAvCarryWeight, 0.01f);
        reinterpret_cast<SetFn>(vt[5])(owner, kAvCarryWeight, -0.01f);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

class SpeedTask : public TaskDelegate {
public:
    void Run() override {
        g_speedQueued = false;
        void* player = ThePlayer();
        if (!player || g_speedBroken) return;
        if (g_baseSpeedMult < 0) {
            float base = SafeGetAv(player, kAvSpeedMult);
            Log("movement: player SpeedMult %.1f", base);
            if (!(base > 1.0f && base < 1000.0f)) {
                g_speedBroken = true;  // not the actor value interface we expect
                Log("movement: unexpected SpeedMult; speed matching disabled");
                return;
            }
            g_baseSpeedMult = base;
            g_speedMult = base;
        }
        float want = g_wantSpeedMult;
        if (want <= 0 || std::fabs(want - g_speedMult) < 0.01f) return;
        if (!SafeSetSpeedMult(player, want)) {
            g_speedBroken = true;
            Log("movement: setting SpeedMult faulted; speed matching disabled");
            return;
        }
        g_speedMult = want;
        if (want == g_baseSpeedMult) Log("movement: SpeedMult restored to %.1f", want);
    }
    void Dispose() override { delete this; }
};

void RequestSpeedMult(float value) {
    g_wantSpeedMult = value;
    if (g_tasks && !g_speedQueued.exchange(true)) g_tasks->AddTask(new SpeedTask());
}

// ---- Snake thread ---------------------------------------------------------------
//
// Snake's controller and the retarget run on this plugin's own thread at a
// fixed 60 Hz. Skyrim calls UpdateAnimation from varying job threads; running
// Rust code there hung the game, so the hook below never calls snake.dll: it
// only reports the player's position and copies the latest finished pose.

std::atomic<bool> g_enabled{false};
std::atomic<int> g_mode{2};  // 0 compute only, 1 local transforms, 2 local + world (default)
std::atomic<float> g_speed{0.0f};  // player speed (m/s), measured by the hook
std::atomic<int> g_snakeStance{0};  // Snake's stance (0 stand, 1 crouch, 2 prone)
std::atomic<bool> g_snakeMoving{false};  // the player is moving (Space dives instead of jumping)
std::atomic<int> g_wantSneak{-1};   // Skyrim's sneak flag to keep (-1: leave it alone)
std::atomic<bool> g_snakeAiming{false};
std::atomic<float> g_cameraYaw{0.0f};      // camera heading (Skyrim convention: forward (sin, cos))
std::atomic<bool> g_cameraYawValid{false};
std::atomic<bool> g_ownHeading{false};      // Snake's facing is shown (NPC Root turned to it)
std::atomic<float> g_snakeHeading{0.0f};    // Snake's facing in Skyrim's heading convention (radians)
std::atomic<float> g_cameraDrop{0.0f};  // camera lowered by (game units) for Snake's head height
std::atomic<float> g_moveForward{0.0f}, g_moveRight{0.0f};  // the same in the actor's frame (m/s)
// Actor::ActorState::actorState1: the ActorState base is at actor + 0xC0 on
// this runtime (vtable there, verified in game), flags follow at + 0xC8.
// Bits 0-3 moving back / forward / right / left, 6 walking, 7 running,
// 8 sprinting, 9 sneaking.
constexpr size_t kActorState1 = 0xC8;
std::atomic<uint32_t> g_actorFlags{0};
std::atomic<float> g_pitch{0.0f};  // camera (or actor) look pitch (radians, positive looks down)
std::atomic<bool> g_running{false};
std::atomic<uint32_t> g_hookCalls{0};  // player UpdateAnimation calls (for the status log)
std::atomic<uint32_t> g_playerFrames{0};  // the same, never reset: tells the Snake thread the game is running
std::thread g_snakeThread;

struct PoseBuffer {
    std::mutex m;
    std::vector<SnakeQuat> rot;
    std::vector<SnakeVec3> tr;
    std::vector<uint8_t> has;
    // Per weapon mesh: 0 hidden, 1 in the hands, 2 holstered.
    int gunPlace[4] = {};
    SnakeVec3 gunPos[4] = {};
    SnakeQuat gunRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};
    // The same for each weapon's magazine (in the gun or Snake's left hand).
    int magPlace[4] = {};
    SnakeVec3 magPos[4] = {};
    SnakeQuat magRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};
    float raised = 0;  // how far the weapon is raised (aiming or reloading)
    bool reloading = false;
    uint64_t seq = 0;
} g_pose;

bool SkyrimFocused() {
    DWORD pid = 0;
    GetWindowThreadProcessId(GetForegroundWindow(), &pid);
    return pid == GetCurrentProcessId();
}

// Edge-triggered key, only while Skyrim has focus. Starts as "down" so a key
// already held (or stuck) when the plugin starts does not toggle anything.
struct Key {
    int vk;
    bool wasDown = true;
    bool Pressed(bool focused) {
        bool down = focused && (GetAsyncKeyState(vk) & 0x8000) != 0;
        bool pressed = down && !wasDown;
        wasDown = down;
        return pressed;
    }
};

// Test/control file: Data/SKSE/Plugins/snake_control.txt, re-read when it
// changes. Lines `key=value`: enabled=0|1, mode=0|1|2, stance=stand|crouch|prone,
// move=auto|idle|walk|run|dash, aim=0|1.
// Camera settings set live from the control file (see the camera section).
std::mutex g_cameraOverrideMutex;
std::map<std::wstring, float> g_cameraOverrides;
std::atomic<bool> g_cameraDirty{false};

struct Control {
    FILETIME stamp{};
    std::string move = "auto";
    int aim = -1;
    int fire = -1;
    std::string stance;  // requested stance, consumed by the Snake thread
    int draw = 1;        // draw hands (face the camera) while aiming
    int flash = 1;       // muzzle flash (diagnostics)
    int launch = 1;      // launch projectiles (diagnostics)
    int weapon = -1;     // switch to this weapon once
    int speed = 1;       // match Skyrim's movement speed to Snake's (when not driving directly)
    int drive = 1;       // Snake moves the player himself (direct drive)
    int trace = 0;       // log Snake's and Skyrim's speeds every 0.1 s (diagnostics)
    int body = 1;        // show Snake's own body
    int reload = 0;      // reload once
    int dive = 0;        // dive once
    float tempo = 0;     // set Snake's tempo once (e.g. 0.3 for slow-motion checks)
    float aimYaw = 0;    // aim this many degrees left of the camera (to see Snake's front or side)
    bool dump = false;   // log the camera once (diagnostics)
    int probe = 0;       // 1: log the player's process / character controller objects once
    int viewModel = 1;   // first person: hold the raised pistol in front of the camera
    // Where (camera space, Skyrim units: right, forward, up) the raised
    // pistol's origin sits in first person; NaN = the built-in default.
    float viewX = NAN, viewY = NAN, viewZ = NAN;
};
Control g_control;

void PollControl(int curStance) {
    std::wstring path = PluginDir() + L"\\snake_control.txt";
    WIN32_FILE_ATTRIBUTE_DATA a{};
    if (!GetFileAttributesExW(path.c_str(), GetFileExInfoStandard, &a)) return;
    if (CompareFileTime(&a.ftLastWriteTime, &g_control.stamp) == 0) return;
    g_control.stamp = a.ftLastWriteTime;
    FILE* f = _wfsopen(path.c_str(), L"r", _SH_DENYNO);
    if (!f) return;
    char line[256];
    while (fgets(line, sizeof line, f)) {
        std::string l(line);
        while (!l.empty() && (l.back() == 0x0A || l.back() == 0x0D || l.back() == 0x20)) l.pop_back();
        auto eq = l.find('=');
        if (eq == std::string::npos) continue;
        std::string k = l.substr(0, eq), v = l.substr(eq + 1);
        if (k == "enabled") g_enabled = v == "1";
        else if (k == "mode") g_mode = std::atoi(v.c_str()) % 3;
        else if (k == "move") g_control.move = v;
        else if (k == "aim") g_control.aim = std::atoi(v.c_str());
        else if (k == "fire") g_control.fire = std::atoi(v.c_str());
        else if (k == "stance") g_control.stance = v;
        else if (k == "draw") g_control.draw = std::atoi(v.c_str());
        else if (k == "flash") g_control.flash = std::atoi(v.c_str());
        else if (k == "launch") g_control.launch = std::atoi(v.c_str());
        else if (k == "weapon") g_control.weapon = std::atoi(v.c_str());
        else if (k == "speed") g_control.speed = std::atoi(v.c_str());
        else if (k == "drive") g_control.drive = std::atoi(v.c_str());
        else if (k == "trace") g_control.trace = std::atoi(v.c_str());
        else if (k == "trackshot") g_trackShot = std::atoi(v.c_str());
        else if (k == "body") g_control.body = std::atoi(v.c_str());
        else if (k == "viewmodel") g_control.viewModel = std::atoi(v.c_str());
        else if (k == "vmx") g_control.viewX = static_cast<float>(std::atof(v.c_str()));
        else if (k == "vmy") g_control.viewY = static_cast<float>(std::atof(v.c_str()));
        else if (k == "vmz") g_control.viewZ = static_cast<float>(std::atof(v.c_str()));
        else if (k == "reload") g_control.reload = std::atoi(v.c_str());
        else if (k == "dive") g_control.dive = std::atoi(v.c_str());
        else if (k == "tempo") g_control.tempo = static_cast<float>(std::atof(v.c_str()));
        else if (k == "aimyaw") g_control.aimYaw = static_cast<float>(std::atof(v.c_str()));
        else if (k.rfind("Camera", 0) == 0 || k.rfind("AimCamera", 0) == 0) {
            std::lock_guard<std::mutex> lock(g_cameraOverrideMutex);
            g_cameraOverrides[std::wstring(k.begin(), k.end())] = static_cast<float>(std::atof(v.c_str()));
            g_cameraDirty = true;
        }
        else if (k == "dump") g_control.dump = v == "1";
        else if (k == "probe") g_control.probe = std::atoi(v.c_str());
    }
    fclose(f);
    Log("control: enabled %d mode %d move %s aim %d stance %s (current %d)", (int)g_enabled, (int)g_mode,
        g_control.move.c_str(), g_control.aim, g_control.stance.c_str(), curStance);
}

// ---- ammo HUD -----------------------------------------------------------------------
//
// Snake mode shows the weapon and its magazine on Skyrim's own HUD (bottom
// right, like MGSV's weapon panel): "AM MRS-4  27 / --" (no reserve count
// with unlimited magazines; the HUD font has no infinity sign). Skyrim's own
// arrow counter beside it is hidden meanwhile. SKSE calls
// RegisterScaleform for every menu movie; in the HUD's, a text field is
// created with the HUD's own font, and UI tasks update it when the text
// changes. Values are passed in GVal, which has GFxValue's layout (SKSE's
// GFxValue methods need its game addresses compiled in; the movie's
// virtual calls do not).

struct GVal {
    void* objectInterface = nullptr;
    uint32_t type = 0;  // 0 undefined, 2 bool, 3 number, 4 string
    uint32_t pad = 0;
    union {
        double number;
        const char* string;
        bool boolean;
        void* object;
    } data{};
};
static_assert(sizeof(GVal) == 0x18, "GFxValue layout");
GVal GNum(double v) { GVal g; g.type = 3; g.data.number = v; return g; }
GVal GStr(const char* v) { GVal g; g.type = 4; g.data.string = v; return g; }
GVal GBool(bool v) { GVal g; g.type = 2; g.data.boolean = v; return g; }
GFxValue* GF(GVal* v) { return reinterpret_cast<GFxValue*>(v); }

SKSEScaleformInterface* g_scaleform = nullptr;
std::atomic<GFxMovieView*> g_hudView{nullptr};
std::mutex g_hudMutex;
std::string g_hudText;      // last text sent
bool g_hudShown = false;

bool RegisterScaleform(GFxMovieView* view, GFxValue*) {
    if (!view) return true;
    GVal probe;
    if (!view->GetVariable(GF(&probe), "_root.HUDMovieBaseInstance") || probe.type == 0) return true;
    GRectF r = view->GetVisibleFrameRect();
    double w = 420, h = 40;
    double x = r.right - w - 60, y = r.bottom - h - 92;
    GVal args[6] = {GStr("snakeAmmo"), GNum(4040), GNum(x), GNum(y), GNum(w), GNum(h)};
    GVal field;
    view->Invoke("_root.createTextField", GF(&field), GF(args), 6);
    GVal fmtArgs[9] = {GStr("$EverywhereMediumFont"), GNum(24), GNum(0xFFFFFF), GBool(false), GBool(false),
                       GBool(false), GVal{}, GVal{}, GStr("right")};
    GVal fmt;
    view->CreateObject(GF(&fmt), "TextFormat", GF(fmtArgs), 9);
    view->Invoke("_root.snakeAmmo.setNewTextFormat", nullptr, GF(&fmt), 1);
    GVal t = GBool(true), f = GBool(false), empty = GStr("");
    view->SetVariable("_root.snakeAmmo.embedFonts", GF(&t), 0);
    view->SetVariable("_root.snakeAmmo.selectable", GF(&f), 0);
    view->SetVariable("_root.snakeAmmo.text", GF(&empty), 0);
    view->SetVariable("_root.snakeAmmo._visible", GF(&f), 0);
    g_hudView = view;
    {
        std::lock_guard<std::mutex> lock(g_hudMutex);
        g_hudText.clear();
        g_hudShown = false;
    }
    GVal arrows;
    bool haveArrows = view->GetVariable(GF(&arrows), "_root.HUDMovieBaseInstance.ArrowInfoInstance") && arrows.type != 0;
    Log("ammo HUD: text field on the HUD movie (visible %.0f,%.0f - %.0f,%.0f); Skyrim's arrow counter %s", r.left, r.top,
        r.right, r.bottom, haveArrows ? "found" : "not found");
    return true;
}

class HudTask : public UIDelegate_v1 {
public:
    HudTask(std::string text, bool show, uint32_t color) : text_(std::move(text)), show_(show), color_(color) {}
    void Run() override {
        GFxMovieView* view = g_hudView;
        if (!view) return;
        GVal t = GStr(text_.c_str()), v = GBool(show_), c = GNum(color_);
        view->SetVariable("_root.snakeAmmo.text", GF(&t), 0);
        view->SetVariable("_root.snakeAmmo.textColor", GF(&c), 0);
        view->SetVariable("_root.snakeAmmo._visible", GF(&v), 0);
        // Skyrim's arrow counter (it shows "(0)" for the drawn hands).
        GVal arrows = GBool(!show_);
        view->SetVariable("_root.HUDMovieBaseInstance.ArrowInfoInstance._visible", GF(&arrows), 0);
    }
    void Dispose() override { delete this; }

private:
    std::string text_;
    bool show_;
    uint32_t color_;
};

// Called on the Snake thread each tick: queue a HUD update when the text or
// visibility changes.
void UpdateHud() {
    if (!g_hudView || !g_tasks) return;
    SnakeStatus st{};
    g_snake.player_status(g_player, &st);
    bool show = g_enabled && st.weapon >= 0;
    std::string text;
    uint32_t color = 0xFFFFFF;
    if (show) {
        const char* name = g_snake.weapon_name(g_runtime, st.weapon);
        char buf[96];
        if (st.unlimited)
            snprintf(buf, sizeof buf, "%s    %d / --", name ? name : "", st.ammo);
        else
            snprintf(buf, sizeof buf, "%s    %d / %d", name ? name : "", st.ammo, st.reserve);
        text = buf;
        if (st.ammo == 0) color = 0xFF5040;               // empty
        else if (st.ammo * 4 <= st.magazine) color = 0xFFC040;  // a quarter left
    }
    std::lock_guard<std::mutex> lock(g_hudMutex);
    // Re-sent twice a second while shown: Skyrim re-shows its arrow counter.
    static int ticks = 0;
    bool refresh = show && ++ticks % 30 == 0;
    if (text == g_hudText && show == g_hudShown && !refresh) return;
    g_hudText = text;
    g_hudShown = show;
    g_tasks->AddUITask(new HudTask(text, show, color));
}

// Skyrim's movement input (PlayerControls::data.moveInputVec, +0x24): x
// right, y forward, -1..1 per axis (keys) or the stick; false if unknown.
bool ReadMoveInput(float& x, float& y) {
    void* pc = g_fire.playerControls && Readable(g_fire.playerControls, 8) ? *g_fire.playerControls : nullptr;
    if (!pc || !Readable(pc, 0x30)) return false;
    x = At<float>(pc, 0x24);
    y = At<float>(pc, 0x28);
    return std::isfinite(x) && std::isfinite(y) && std::fabs(x) <= 1.5f && std::fabs(y) <= 1.5f;
}

// ---- direct drive: Snake moves the character ------------------------------------
//
// Skyrim's movement code sets the player's Havok character controller
// velocity once per frame through bhkCharacterController::
// SetLinearVelocityImpl (vtable slot 7 of the bhkCharProxyController,
// found by counting per-frame calls: slot 7 receives Skyrim's world velocity
// in Havok units, metres per second). In Snake mode its horizontal part is
// replaced by Snake's own root motion (MGSV's starts, stops and strafes, at
// MGSV's speeds); the vertical part (gravity, slopes) and collision stay
// Skyrim's. The vtable is shared by every character, so only the player's
// controller is changed. Snake's velocity comes from the Snake thread, in
// Skyrim's world axes.

constexpr int kVtSetLinearVelocity = 7;
using SetVelocityFn = void (*)(void* self, const float* velocity);
SetVelocityFn g_originalSetVelocity = nullptr;
std::atomic<void*> g_playerController{nullptr};
std::atomic<bool> g_driving{false};         // Snake mode with a fresh velocity
std::atomic<float> g_driveVx{0}, g_driveVy{0};  // m/s, Skyrim world X / Y
std::atomic<int64_t> g_driveStamp{0};       // NowMs() of the last update
std::atomic<bool> g_driveQueued{false};

void HookedSetVelocity(void* self, const float* velocity) {
    if (g_driving && self == g_playerController && velocity && NowMs() - g_driveStamp < 200) {
        alignas(16) float v[4] = {g_driveVx, g_driveVy, velocity[2], velocity[3]};
        g_originalSetVelocity(self, v);
        return;
    }
    g_originalSetVelocity(self, velocity);
}

// Hook slot 7 once a player controller exists (main thread).
class DriveHookTask : public TaskDelegate {
public:
    void Run() override {
        g_driveQueued = false;
        void* cc = CharController(ThePlayer());
        if (!cc) return;
        g_playerController = cc;
        if (g_originalSetVelocity) return;
        void** vt = Vtable(cc);
        DWORD old;
        if (!VirtualProtect(&vt[kVtSetLinearVelocity], sizeof(void*), PAGE_EXECUTE_READWRITE, &old)) return;
        g_originalSetVelocity = reinterpret_cast<SetVelocityFn>(vt[kVtSetLinearVelocity]);
        vt[kVtSetLinearVelocity] = reinterpret_cast<void*>(&HookedSetVelocity);
        VirtualProtect(&vt[kVtSetLinearVelocity], sizeof(void*), old, &old);
        Log("direct drive: hooked %s::SetLinearVelocityImpl (vtable %p); Snake moves the player", RttiName(cc), vt);
    }
    void Dispose() override { delete this; }
};

// From the Snake thread each tick: Snake's horizontal velocity (his root
// motion over the tick, in his own frame) turned into Skyrim's world axes
// by his facing h (forward (sin h, cos h), right (cos h, -sin h)).
void UpdateDrive(void* player, float forward, float right, float h) {
    void* cc = CharController(player);
    if (cc != g_playerController) {
        g_playerController = cc;
        if (cc && !g_originalSetVelocity && g_tasks && !g_driveQueued.exchange(true)) g_tasks->AddTask(new DriveHookTask());
    }
    g_driveVx = forward * std::sin(h) + right * std::cos(h);
    g_driveVy = forward * std::cos(h) - right * std::sin(h);
    g_driveStamp = NowMs();
    g_driving = cc != nullptr;
}

// ---- crawling: Sneak bonus ----------------------------------------------------------
//
// Prone, Snake gets a hidden Sneak bonus (snake.ini CrawlSneakBonus, default
// +20: MGSV keeps its stance camouflage values in code, not data, so the
// requested fallback). It is an actor value modifier (Sneak = actor value
// 15, checked in game), added on the main thread, taken off when he gets
// up or Snake mode ends, and taken off before every save so it never ends
// up in one.

constexpr uint32_t kAvSneak = 15;
std::atomic<bool> g_crawlBonusWanted{false};
bool g_crawlBonusApplied = false;  // main thread
std::atomic<bool> g_crawlBonusQueued{false};

bool SafeModAv(void* actor, uint32_t av, float delta) {
    __try {
        void* owner = reinterpret_cast<char*>(actor) + kActorValueOwner;
        using ModFn = void (*)(void*, uint32_t, float);
        reinterpret_cast<ModFn>(Vtable(owner)[5])(owner, av, delta);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

void ApplyCrawlBonus(bool on) {
    void* player = ThePlayer();
    if (!player || on == g_crawlBonusApplied) return;
    float bonus = static_cast<float>(IniInt(L"CrawlSneakBonus", 20));
    if (SafeModAv(player, kAvSneak, on ? bonus : -bonus)) {
        g_crawlBonusApplied = on;
        Log("crawling: Sneak %+.0f (now %.0f)", on ? bonus : -bonus, SafeGetAv(player, kAvSneak));
    }
}

class CrawlBonusTask : public TaskDelegate {
public:
    void Run() override {
        g_crawlBonusQueued = false;
        ApplyCrawlBonus(g_crawlBonusWanted);
    }
    void Dispose() override { delete this; }
};

void RequestCrawlBonus(bool on) {
    g_crawlBonusWanted = on;
    if (on != g_crawlBonusApplied && g_tasks && !g_crawlBonusQueued.exchange(true)) g_tasks->AddTask(new CrawlBonusTask());
}

// ---- sneaking: Skyrim's own sneak toggle ----------------------------------------
//
// Snake's stance decides whether the player sneaks, but the toggle itself is
// Skyrim's: its SneakHandler sets the actor's sneak flag, the animation graph
// and the HUD's sneak eye together. Writing the flag alone left the eye out
// of step (open after standing up, shut after diving prone), and the graph
// events "sneakStart"/"sneakStop" did not fix it. Ctrl is blocked from the
// handler in Snake mode (it sets Snake's stance), so the hook keeps a copy of
// the first real "Sneak" button event; until then one is built from the
// first button event of any key (seen by the attack handler's hook) with
// the pooled "Sneak" control id and Ctrl's key code. Whenever Skyrim's sneak state
// differs from Snake's the copy is replayed (as a fresh press) through the
// handler's own ProcessButton on the main thread: vtable slot 6 on this
// runtime (slots 2-5 are the base class's shared no-ops; CommonLib's slot 4
// is SE's). Until Ctrl has been pressed once there is no copy, and the flag
// is written directly. A replay that changes nothing (Skyrim refuses a
// toggle now and then, e.g. right after the previous one) is retried every
// 0.3 s, and none is tried while hands are drawn or sheathed (Skyrim refuses
// then, and for a moment after); only after 20 refusals in a row (6 s) is
// the flag written directly.

std::atomic<int> g_sneakSyncWanted{-1};
std::atomic<bool> g_sneakSyncQueued{false};
std::atomic<bool> g_haveSneakTemplate{false};
std::atomic<bool> g_sneakToggleBroken{false};  // replays keep changing nothing: write the flag instead
int g_sneakToggleFailures = 0;                  // replays in a row that changed nothing (main thread)
std::mutex g_sneakTemplateMutex;
uint8_t g_sneakTemplate[0x30];
void* g_sneakHandler = nullptr;

// From the sneak handler's CanProcess: keep the first "Sneak" button event.
bool g_sneakTemplateReal = false;  // the template is a real "Sneak" event (input threads)

void CaptureSneakEvent(void* handler, void* event) {
    if (g_sneakTemplateReal || !event) return;
    const char* name = RttiName(event);
    if (!name || !strstr(name, "ButtonEvent") || !Readable(event, 0x30)) return;
    const char* id = At<const char*>(event, 0x18);  // IDEvent control id (BSFixedString)
    if (!id || !Readable(id, 6) || strcmp(id, "Sneak") != 0) return;
    {
        std::lock_guard<std::mutex> lock(g_sneakTemplateMutex);
        memcpy(g_sneakTemplate, event, sizeof g_sneakTemplate);
        g_sneakHandler = handler;
    }
    g_sneakTemplateReal = true;
    g_haveSneakTemplate = true;
    Log("sneak: kept Skyrim's Sneak button event; sneaking now toggles through its own handler");
}

// From the attack handler's CanProcess (it is offered every input event):
// until a real "Sneak" event is seen, build one from any button event.
void BuildSneakEvent(void* event) {
    if (g_haveSneakTemplate || !event || !g_sneakHandler || !g_fire.projectile) return;
    const char* name = RttiName(event);
    if (!name || !strstr(name, "ButtonEvent") || !Readable(event, 0x30)) return;
    const char* sneakId = SafePooled("Sneak", At<const char*>(g_fire.projectile, kModelPath));
    if (!sneakId) return;
    {
        std::lock_guard<std::mutex> lock(g_sneakTemplateMutex);
        memcpy(g_sneakTemplate, event, sizeof g_sneakTemplate);
        At<uint32_t>(g_sneakTemplate, 0x08) = 0;              // keyboard
        At<uint32_t>(g_sneakTemplate, 0x0C) = 0;              // button event
        At<const char*>(g_sneakTemplate, 0x18) = sneakId;     // control id
        At<uint32_t>(g_sneakTemplate, 0x20) = 0x1D;           // Left Ctrl
    }
    g_haveSneakTemplate = true;
    Log("sneak: built a Sneak button event; sneaking toggles through Skyrim's own handler");
}

void CopySneakTemplate(uint8_t* out) {
    std::lock_guard<std::mutex> lock(g_sneakTemplateMutex);
    memcpy(out, g_sneakTemplate, sizeof g_sneakTemplate);
}

bool SafeToggleSneak(void* handler, uint8_t* event, void* controlsData) {
    __try {
        using ProcessButtonFn = void (*)(void*, void*, void*);
        reinterpret_cast<ProcessButtonFn>(Vtable(handler)[6])(handler, event, controlsData);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

class SneakSyncTask : public TaskDelegate {
public:
    void Run() override {
        g_sneakSyncQueued = false;
        int want = g_sneakSyncWanted;
        void* player = ThePlayer();
        void* controls = g_fire.playerControls && Readable(g_fire.playerControls, 8) ? *g_fire.playerControls : nullptr;
        if (want < 0 || !player || !controls || !g_haveSneakTemplate || g_sneakToggleBroken) return;
        int before = (At<uint32_t>(player, kActorState1) >> 9) & 1;
        if (before == want) return;
        // Skyrim refuses sneak toggles while hands are drawn or sheathed
        // (after aiming) and for a moment after: wait for that to end.
        int weapon = WeaponStateOf(player);
        if (weapon != kSheathed && weapon != kDrawn) return;
        alignas(16) uint8_t event[0x30];
        CopySneakTemplate(event);
        At<void*>(event, 0x10) = nullptr;  // next event
        At<float>(event, 0x28) = 1.0f;     // value: pressed
        At<float>(event, 0x2C) = 0.0f;     // held for: just now
        bool ok = SafeToggleSneak(g_sneakHandler, event, reinterpret_cast<char*>(controls) + 0x24);
        int after = (At<uint32_t>(player, kActorState1) >> 9) & 1;
        Log("sneak: Skyrim's toggle %s, sneaking %d -> %d (want %d, weapon state %d)", ok ? "ran" : "faulted", before, after, want,
            WeaponStateOf(player));
        if (after != before) {
            g_sneakToggleFailures = 0;
        } else if (++g_sneakToggleFailures >= 20) {
            g_sneakToggleBroken = true;
            Log("sneak: Skyrim's toggle keeps having no effect; writing the sneak flag directly from now on");
        }
    }
    void Dispose() override { delete this; }
};

void RequestSneakSync(int sneak) {
    g_sneakSyncWanted = sneak;
    if (g_tasks && !g_sneakSyncQueued.exchange(true)) g_tasks->AddTask(new SneakSyncTask());
}

// ---- camera -----------------------------------------------------------------------
//
// MGSV's third-person camera: a fixed distance behind Snake (no zooming), a
// little to his right and above, and aiming close over his right shoulder.
// Skyrim's camera reads its [Camera] INI settings live, so in Snake mode the
// plugin sets them and restores the player's own values afterwards:
// distance = fVanityModeMinDist = fVanityModeMaxDist (the zoom range, so the
// wheel no longer changes it), side / height = fOverShoulderPosX / PosZ,
// aiming (drawn hands) = fOverShoulderCombatPosX / PosZ, pulled in by
// fOverShoulderCombatAddY (positive pulls in; Skyrim reads the aiming values
// when the hands are drawn). Values from snake.ini (Camera* / AimCamera* keys,
// game units; AimCamera=0 leaves the camera alone); the control file can
// override them live for tuning (same key names).
// The settings are found by name: an INI Setting is {vtable, value, name},
// so the object is the one in the exe's data whose +0x10 points at the
// name string.

struct CameraSetting {
    const char* name;      // Skyrim INI setting
    const wchar_t* key;    // snake.ini key
    float fallback;        // MGSV-like default
    FloatSetting setting;
};
CameraSetting g_camera[] = {
    {"fVanityModeMinDist:Camera", L"CameraDistance", 180, {}},
    {"fVanityModeMaxDist:Camera", L"CameraDistance", 180, {}},
    {"fOverShoulderPosX:Camera", L"CameraX", 35, {}},
    {"fOverShoulderPosZ:Camera", L"CameraZ", 12, {}},
    {"fOverShoulderCombatPosX:Camera", L"AimCameraX", 50, {}},
    {"fOverShoulderCombatPosZ:Camera", L"AimCameraZ", -10, {}},
    {"fOverShoulderCombatAddY:Camera", L"AimCameraAddY", 135, {}},
};


bool InImage(uintptr_t p, uintptr_t base, size_t size) { return p >= base && p < base + size; }

float* FindFloatSetting(const char* name) {
    auto base = reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr));
    auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
    auto nt = reinterpret_cast<IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
    size_t imageSize = nt->OptionalHeader.SizeOfImage;
    IMAGE_SECTION_HEADER* first = IMAGE_FIRST_SECTION(nt);
    size_t len = strlen(name) + 1;  // with the terminator
    std::vector<uintptr_t> strings;
    for (int i = 0; i < nt->FileHeader.NumberOfSections; ++i) {
        const IMAGE_SECTION_HEADER& sec = first[i];
        if (sec.Characteristics & IMAGE_SCN_MEM_EXECUTE) continue;
        auto* p = reinterpret_cast<const char*>(base + sec.VirtualAddress);
        size_t n = sec.Misc.VirtualSize;
        for (size_t k = 1; k + len <= n; ++k)
            if (p[k] == name[0] && p[k - 1] == 0 && memcmp(p + k, name, len) == 0) strings.push_back(reinterpret_cast<uintptr_t>(p + k));
    }
    for (uintptr_t str : strings) {
        for (int i = 0; i < nt->FileHeader.NumberOfSections; ++i) {
            const IMAGE_SECTION_HEADER& sec = first[i];
            if (!(sec.Characteristics & IMAGE_SCN_MEM_WRITE)) continue;
            auto* q = reinterpret_cast<const uintptr_t*>(base + sec.VirtualAddress);
            size_t n = sec.Misc.VirtualSize / sizeof(uintptr_t);
            for (size_t k = 2; k < n; ++k) {
                if (q[k] != str || !InImage(q[k - 2], base, imageSize)) continue;
                float* value = reinterpret_cast<float*>(const_cast<uintptr_t*>(q + k - 1));
                if (std::isfinite(*value) && std::fabs(*value) < 10000.0f) return value;
            }
        }
    }
    return nullptr;
}

bool SafeFindFloatSetting(const char* name, FloatSetting& out);

// Diagnostics (probe=7): every float INI setting of a section ("f...:Camera")
// with its live value.
void LogFloatSettings(const char* section) {
    auto base = reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr));
    auto dos = reinterpret_cast<IMAGE_DOS_HEADER*>(base);
    auto nt = reinterpret_cast<IMAGE_NT_HEADERS64*>(base + dos->e_lfanew);
    IMAGE_SECTION_HEADER* first = IMAGE_FIRST_SECTION(nt);
    std::string suffix = std::string(":") + section;
    std::vector<std::string> names;
    for (int i = 0; i < nt->FileHeader.NumberOfSections; ++i) {
        const IMAGE_SECTION_HEADER& sec = first[i];
        if (sec.Characteristics & IMAGE_SCN_MEM_EXECUTE) continue;
        auto* p = reinterpret_cast<const char*>(base + sec.VirtualAddress);
        size_t n = sec.Misc.VirtualSize;
        for (size_t k = 1; k + 4 < n; ++k) {
            if (p[k] != 'f' || p[k - 1] != 0) continue;
            size_t e = k;
            while (e < n && e - k < 80 && p[e] >= 0x20 && p[e] < 0x7F) ++e;
            if (e >= n || p[e] != 0) continue;
            std::string str(p + k, e - k);
            if (str.size() > suffix.size() && str.compare(str.size() - suffix.size(), suffix.size(), suffix) == 0) names.push_back(str);
        }
    }
    for (const std::string& name : names) {
        FloatSetting f;
        if (SafeFindFloatSetting(name.c_str(), f)) Log("setting %s = %.3f", name.c_str(), *f.value);
    }
    Log("settings: %zu names in section %s", names.size(), section);
}

bool SafeFindFloatSetting(const char* name, FloatSetting& out) {
    __try {
        out.value = FindFloatSetting(name);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        out.value = nullptr;
    }
    if (out.value) out.original = *out.value;
    return out.value != nullptr;
}

void SetUpAimCamera() {
    if (!IniInt(L"AimCamera", 1)) {
        Log("camera: Skyrim's own (snake.ini AimCamera=0)");
        return;
    }
    for (CameraSetting& c : g_camera) {
        bool found = SafeFindFloatSetting(c.name, c.setting);
        Log("camera: %s %s (%.1f)", c.name, found ? "found" : "not found", found ? c.setting.original : 0.0f);
    }
}

float CameraValue(const CameraSetting& c) {
    {
        std::lock_guard<std::mutex> lock(g_cameraOverrideMutex);
        auto it = g_cameraOverrides.find(c.key);
        if (it != g_cameraOverrides.end()) return it->second;
    }
    std::wstring ini = PluginDir() + L"\\snake.ini";
    wchar_t buf[32] = {};
    GetPrivateProfileStringW(L"Snake", c.key, L"", buf, 32, ini.c_str());
    return buf[0] ? static_cast<float>(_wtof(buf)) : c.fallback;
}

// Snake's aim camera on (Snake mode) or the player's own values back.
void ApplyAimCamera(bool snake) {
    static bool applied = false;
    bool dirty = g_cameraDirty.exchange(false);
    if (snake == applied && !dirty) return;
    applied = snake;
    for (CameraSetting& c : g_camera)
        if (c.setting.value) *c.setting.value = snake ? CameraValue(c) : c.setting.original;
    if (snake && dirty) {
        std::string row;
        char b[64];
        for (const CameraSetting& c : g_camera) {
            snprintf(b, sizeof b, " %ls=%.0f", c.key, c.setting.value ? *c.setting.value : -1.0f);
            row += b;
        }
        Log("camera:%s", row.c_str());
    }
}

void SnakeThread() {
    // Crouch follows Skyrim's sneak; X toggles prone (C is Skyrim's
    // auto-move, Z its shout key).
    Key f8{VK_F8}, f9{VK_F10};
    const int sprintVk = IniInt(L"SprintKey", VK_LMENU);
    // Space while running or dashing: MGSV's dive to prone.
    Key diveKey{VK_SPACE};
    // 1 / 2 switch Snake's weapon (MRS-4 / WU S.Pistol), as in MGSV's
    // primary/secondary slots.
    Key weaponKeys[2] = {{'1'}, {'2'}};
    // R reloads, as in MGSV (Skyrim's ready-weapon on R is off in Snake mode).
    Key reloadKey{'R'};
    // Stance from Ctrl (Skyrim's sneak key), as in MGSV: a tap goes up one
    // stance (prone -> crouch, crouch <-> stand); holding goes prone, or
    // from prone stands up. Skyrim's own sneak toggle is off in Snake mode
    // and its sneak flag follows Snake (crouched or prone = sneaking).
    int wantStance = -1;  // -1: take Skyrim's sneak state when Snake mode starts
    double ctrlHeld = -1;  // seconds Ctrl has been down (-1: up)
    bool ctrlHoldDone = false;
    constexpr double kHoldSeconds = 0.35;
    float standHead = 1.6f;  // Snake's standing head height (m), learned
    bool owning = false;     // Snake sets the actor's heading (see "Facing" below)
    float headingSum = 0;    // c in h = c - y
    double sinceAim = 10.0;  // seconds since Snake last aimed
    float snakeAvg = 0, skyAvg = 0;  // smoothed ground speeds (m/s): Snake's, Skyrim's
    double speedTimer = 0, movingTime = 0;
    // Skyrim ground speed at SpeedMult 100 per gait (walk, run, sprint; m/s):
    // run measured in game, the others first guesses, all refined in play.
    float skyBase[3] = {1.6f, 5.0f, 7.0f};
    float snakeSpeedOf[12] = {};  // Snake's steady speed per stance x gait (0: not seen yet)
    int lastState = -1;
    uint32_t lastDir = 0;
    double sinceSpeedChange = 10.0;
    bool handsDrawn = false;
    uint32_t lastFlags = 0xFFFFFFFF;
    uint32_t lastFrames = 0;
    int stalledTicks = 0;
    bool paused = false;
    const double dt = 1.0 / 60.0;
    auto next = std::chrono::steady_clock::now();
    double logTimer = 0;
    uint32_t n = g_snake.retarget_count(g_retarget);
    std::vector<SnakeQuat> rot(n);
    std::vector<SnakeVec3> tr(n);
    std::vector<uint8_t> has(n);
    Log("snake thread %u running at 60 Hz", GetCurrentThreadId());
    while (g_running) {
        next += std::chrono::microseconds(16667);
        std::this_thread::sleep_until(next);
        bool focused = SkyrimFocused();
        SnakeStatus cur{};
        g_snake.player_status(g_player, &cur);
        PollControl(cur.stance);
        if (g_control.probe == 7) {
            g_control.probe = 0;
            LogFloatSettings("Camera");
        }
        if (g_control.probe == 6 && g_tasks) {
            g_control.probe = 0;
            g_tasks->AddTask(new MemDumpTask());
        }
        if (f9.Pressed(focused)) {
            int m = (g_mode + 1) % 3;
            g_mode = m;
            Log("write mode %d (%s)", m, m == 0 ? "compute only" : m == 1 ? "local transforms" : "local + world transforms");
        }
        if (f8.Pressed(focused)) {
            g_enabled = !g_enabled;
            Log("Snake mode %s", g_enabled ? "ON" : "OFF");
        }
        ApplyAimCamera(g_enabled);
        if (!g_enabled) {
            // Skyrim's own sneak toggle, camera and Sneak again.
            wantStance = -1;
            g_wantSneak = -1;
            owning = false;
            g_ownHeading = false;
            g_cameraDrop = 0.0f;
            RequestCrawlBonus(false);
            // Give the player their own speed back.
            float base = g_baseSpeedMult;
            if (base > 0 && g_speedMult > 0 && std::fabs(g_speedMult - base) > 0.01f) RequestSpeedMult(base);
            continue;
        }
        // Hold Snake still while the game is not updating the player (menus,
        // tutorial pop-ups, loading): otherwise his controller runs on and
        // fires into a frozen world.
        uint32_t frames = g_playerFrames.load(std::memory_order_relaxed);
        stalledTicks = frames == lastFrames ? stalledTicks + 1 : 0;
        lastFrames = frames;
        if ((stalledTicks > 6) != paused) {
            paused = stalledTicks > 6;
            Log("game %s; Snake %s", paused ? "paused" : "running", paused ? "holds" : "resumes");
        }
        if (paused) continue;

        float speed = g_speed;
        uint32_t flags = g_actorFlags;
        if (flags != lastFlags) {
            Log("actor state: moving %x walk %u run %u sprint %u sneak %u (speed %.2f m/s)", flags & 0xF, (flags >> 6) & 1,
                (flags >> 7) & 1, (flags >> 8) & 1, (flags >> 9) & 1, speed);
            lastFlags = flags;
        }
        // While speed matching, the measured speed depends on SpeedMult, so
        // movement and gait come from Skyrim's input flags alone (deriving
        // them from the speed fed back on itself and stalled the player).
        bool matching = g_control.speed && !g_speedBroken;
        bool moving = (flags & 0xF) != 0 || (!matching && speed > 0.3f);
        // Dash: held, as in MGSV, on Skyrim's sprint key (snake.ini SprintKey,
        // a virtual-key code, default Left Alt). Skyrim's own sprint is off
        // in Snake mode: it drained stamina and stayed latched after the key
        // was released.
        bool sprint = focused && (GetAsyncKeyState(sprintVk) & 0x8000) != 0;
        bool walkFlag = (flags >> 6) & 1, runFlag = (flags >> 7) & 1;
        bool sneaking = (flags >> 9) & 1;
        const std::string& mv = g_control.move;
        if (mv == "idle") moving = false, sprint = false;
        else if (mv == "walk") moving = true, walkFlag = true, runFlag = sprint = false;
        else if (mv == "run") moving = true, runFlag = true, walkFlag = sprint = false;
        else if (mv == "dash") moving = true, sprint = true;
        SnakeInput in{};
        in.switch_to = -1;
        for (int w = 0; w < 2; ++w)
            if (weaponKeys[w].Pressed(focused)) in.switch_to = w;
        if (g_control.tempo > 0) {
            g_snake.player_set_tempo(g_player, g_control.tempo);
            g_control.tempo = 0;
        }
        if (g_control.weapon >= 0) {
            in.switch_to = g_control.weapon;
            g_control.weapon = -1;
        }
        // Gait from Skyrim's own flags (falling back to the measured speed
        // when not speed matching).
        if (sprint && moving) in.sprint = true;
        else if (walkFlag || (!matching && !runFlag && speed < 2.6f)) in.walk = true;
        if (wantStance < 0) wantStance = sneaking ? 1 : 0;
        bool ctrlDown = focused && (GetAsyncKeyState(VK_CONTROL) & 0x8000) != 0;
        if (ctrlDown) {
            if (ctrlHeld < 0) {
                ctrlHeld = 0;
                ctrlHoldDone = false;
            } else {
                ctrlHeld += dt;
            }
            if (!ctrlHoldDone && ctrlHeld >= kHoldSeconds) {
                ctrlHoldDone = true;
                wantStance = wantStance == 2 ? 0 : 2;
                Log("stance: Ctrl held -> %s", wantStance == 2 ? "prone" : "stand");
            }
        } else if (ctrlHeld >= 0) {
            if (!ctrlHoldDone) {
                // From the stance Snake is heading for, so quick taps chain
                // (prone -> crouch -> stand) while a transition still plays.
                wantStance = wantStance == 2 ? 1 : wantStance == 1 ? 0 : 1;
                Log("stance: Ctrl tapped -> %s", wantStance == 1 ? "crouch" : "stand");
            }
            ctrlHeld = -1;
        }
        // The control file's stance is a one-off request.
        if (g_control.stance == "stand") wantStance = 0;
        else if (g_control.stance == "crouch") wantStance = 1;
        else if (g_control.stance == "prone") wantStance = 2;
        g_control.stance.clear();
        int want = wantStance;
        g_snakeStance = want;
        RequestCrawlBonus(cur.stance == 2);
        {
            // Skyrim's own toggle whenever its sneak state differs (also
            // after Skyrim ends sneaking by itself), at most every 0.3 s.
            int sneak = want >= 1 ? 1 : 0;
            g_wantSneak = sneak;
            static float sinceSync = 1.0f;
            sinceSync += static_cast<float>(dt);
            void* pl = ThePlayer();
            if (pl && g_haveSneakTemplate && !g_sneakToggleBroken && sinceSync > 0.3f &&
                static_cast<int>((At<uint32_t>(pl, kActorState1) >> 9) & 1) != sneak) {
                sinceSync = 0.0f;
                RequestSneakSync(sneak);
            }
        }
        // Camera follows Snake's head down (crouched, prone), smoothed.
        {
            SnakeStatus hs{};
            g_snake.player_status(g_player, &hs);
            if (hs.stance == 0 && hs.gait == 0 && !hs.aiming && hs.head_height > 1.0f)
                standHead += (hs.head_height - standHead) * 0.02f;
            float target = (std::min)(0.0f, (hs.head_height - standHead) * 70.0f);
            float k = 1.0f - std::exp(-static_cast<float>(dt) / 0.15f);
            g_cameraDrop = g_cameraDrop + (target - g_cameraDrop) * k;
        }
        // Dashing from a crouch stands Snake up (he rises into the dash);
        // prone the dash key rolls him sideways (with left or right) and
        // otherwise he keeps crawling (snake.dll decides).
        if (in.sprint && cur.stance == 1 && want == 1) wantStance = want = 0;
        bool risingIntoDash = in.sprint && cur.stance == 1 && want == 0;
        if (want != cur.stance && !risingIntoDash) {
            if (want == 2) in.prone = true;                         // stand/crouch -> prone
            else if (cur.stance == 2) (want == 1 ? in.crouch : in.prone) = true;  // prone -> crouch / stand
            else in.crouch = true;                                  // stand <-> crouch
        }
        in.aim = g_control.aim >= 0 ? g_control.aim == 1 : focused && (GetAsyncKeyState(VK_RBUTTON) & 0x8000) != 0;
        g_snakeAiming = in.aim;
        in.fire = in.aim && (g_control.fire >= 0 ? g_control.fire == 1 : focused && (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0);
        in.reload = reloadKey.Pressed(focused) || g_control.reload != 0;
        // Dive: Space while moving, standing or crouched, not aiming. Snake
        // lands prone, so the stance Ctrl keeps is prone from now on (the
        // controller ignores stance requests while the dive plays).
        bool divePressed = diveKey.Pressed(focused) || g_control.dive != 0;
        g_control.dive = 0;
        g_snakeMoving = moving;
        if (divePressed && moving && !in.aim && cur.stance != 2) {
            in.dive = 1;
            wantStance = 2;
            Log("dive");
        }
        g_control.reload = 0;
        in.aim_pitch = -g_pitch.load();
        // Direction: while aiming the actor faces the camera and strafes, so
        // Skyrim's movement input (PlayerControls +0x24: x right, y forward,
        // keys or stick) is Snake's stick; otherwise Skyrim turns the actor
        // to its heading and Snake moves straight ahead. (The actor's movement
        // flags cannot tell back from right while aiming.)
        if (moving) {
            float mx = 0, my = 1;
            float ix = 0, iy = 0;
            if (in.aim && ReadMoveInput(ix, iy)) {
                float len = std::sqrt(ix * ix + iy * iy);
                if (len > 0.05f) {
                    mx = ix / (std::max)(len, 1.0f);
                    my = iy / (std::max)(len, 1.0f);
                }
            }
            in.move_x = mx;
            in.move_y = my;
        }
        // Facing: Snake's yaw y and a Skyrim heading h turn opposite ways
        // (Fox +X is Skyrim -X), so h = c - y, with c fixed from when Snake
        // mode starts. Aiming, Snake turns toward the camera and the stick
        // strafes relative to it; otherwise he steers toward camera heading
        // + stick direction (and stays put when there is no stick).
        SnakeVec3 snakePos{};
        float snakeYaw = 0;
        g_snake.player_transform(g_player, &snakePos, &snakeYaw);
        in.camera_yaw = snakeYaw;
        void* playerRef = ThePlayer();
        if (!owning && playerRef) {
            headingSum = snakeYaw + At<float>(playerRef, kRefAngle + 8);
            owning = true;
        }
        float ix = 0, iy = 0;
        bool stick = ReadMoveInput(ix, iy) && ix * ix + iy * iy > 0.01f;
        if (owning && g_cameraYawValid) {
            if (in.aim) {
                in.camera_yaw = headingSum - g_cameraYaw + g_control.aimYaw * 0.0174533f;
            } else if (moving && stick) {
                in.camera_yaw = headingSum - (g_cameraYaw + std::atan2(ix, iy));
                in.move_x = 0;
                in.move_y = (std::min)(1.0f, std::sqrt(ix * ix + iy * iy));
            }
        }
        SnakeVec3 before = snakePos;
        if (g_tracked.active && g_tasks) g_tasks->AddTask(new TrackTask());
        if (!g_snake.player_update(g_player, &in, nullptr, static_cast<float>(dt))) {
            Log("snake_player_update failed: %s", g_snake.last_error());
            g_enabled = false;
            continue;
        }
        {
            // Speed matching: Snake's own ground speed vs Skyrim's.
            SnakeVec3 after{};
            float yawAfter = 0;
            g_snake.player_transform(g_player, &after, &yawAfter);
            float dx = after.x - before.x, dz = after.z - before.z;
            float snakeSpeed = std::sqrt(dx * dx + dz * dz) / static_cast<float>(dt);
            float snakeHeading = headingSum - yawAfter;
            if (owning) {
                g_snakeHeading = snakeHeading;
                g_ownHeading = true;
            } else {
                g_ownHeading = false;
            }
            // Snake's own frame (Fox: +Z ahead at yaw 0, right is -X).
            float fwd = (dx * std::sin(yawAfter) + dz * std::cos(yawAfter)) / static_cast<float>(dt);
            float rgt = (-dx * std::cos(yawAfter) + dz * std::sin(yawAfter)) / static_cast<float>(dt);
            if (void* player = ThePlayer(); player && g_control.drive && owning) UpdateDrive(player, fwd, rgt, snakeHeading);
            else g_driving = false;
            bool driven = g_driving && g_originalSetVelocity;
            static int traceTick = 0;
            if (g_control.trace) {
                static std::string lastClip;
                char clip[96] = {};
                g_snake.player_clip(g_player, clip, sizeof clip);
                if (lastClip != clip) {
                    Log("trace: clip %s (flags %x, moving %d, walk %d run %d sprint %d)", clip, flags & 0xF, (int)moving,
                        (int)in.walk, (int)!in.walk && !in.sprint, (int)in.sprint);
                    lastClip = clip;
                }
            }
            if (g_control.trace && ++traceTick % 6 == 0)
                Log("trace: snake %.2f fwd %.2f right %.2f | skyrim %.2f fwd %.2f right %.2f m/s, heading %.0f deg (stance %d, moving %d, driven %d)",
                    snakeSpeed, fwd, rgt, speed, g_moveForward.load(), g_moveRight.load(),
                    ThePlayer() ? At<float>(ThePlayer(), kRefAngle + 8) * 57.2958f : 0.0f, (int)cur.stance, (int)moving,
                    (int)driven);
            if (g_control.trace && traceTick % 6 == 0 && ThePlayer()) {
                // Headings of the 3D root and its NPC node (local +Y in world).
                auto yawOf = [](void* n) {
                    if (!n || !Readable(n, 0x140)) return -1.0f;
                    const float* m = &At<float>(n, kWorldRot);
                    float a = std::atan2(m[1], m[4]) * 57.2958f;
                    return a < 0 ? a + 360.0f : a;
                };
                void* root = Get3D(ThePlayer(), false);
                Log("trace: root yaw %.0f, NPC yaw %.0f, NPC Root yaw %.0f, input %.2f %.2f", yawOf(root), yawOf(root ? FindNode(root, "NPC") : nullptr), yawOf(root ? FindNode(root, "NPC Root [Root]") : nullptr),
                    [] { float x = 0, y = 0; ReadMoveInput(x, y); return x; }(), [] { float x = 0, y = 0; ReadMoveInput(x, y); return y; }());
            }
            // SpeedMult = 100 x Snake's speed / Skyrim's speed at SpeedMult
            // 100, both learned per state: Snake's per stance x gait (its
            // controller is deterministic, so this settles quickly), Skyrim's
            // per Skyrim gait from steady stretches only (moving for 1.5 s,
            // SpeedMult unchanged for 1 s, speed close to its average). Direct
            // feedback on the noisy measured speed oscillated.
            SnakeStatus now{};
            g_snake.player_status(g_player, &now);
            int state = std::clamp(now.stance, 0, 2) * 4 + std::clamp(now.gait, 0, 3);
            int skyGait = ((flags >> 8) & 1) ? 2 : ((flags >> 6) & 1) ? 0 : 1;  // sprint / walk / run
            // A new direction (turning round) or state restarts the steady
            // stretch.
            if (moving && state == lastState && (flags & 0xF) == lastDir) {
                movingTime += dt;
                snakeAvg += (snakeSpeed - snakeAvg) * 0.1f;
                skyAvg += (speed - skyAvg) * 0.1f;
            } else {
                movingTime = 0;
                snakeAvg = skyAvg = 0;
            }
            lastState = state;
            lastDir = flags & 0xF;
            sinceSpeedChange += dt;
            float cur = g_speedMult.load();
            if (moving && movingTime > 1.0) snakeSpeedOf[state] = snakeAvg;
            if (cur > 0 && moving && movingTime > 1.5 && sinceSpeedChange > 1.0 &&
                std::fabs(speed - skyAvg) < 0.15f * skyAvg && skyAvg > 0.2f) {
                // Outliers (running into a wall, turning) are ignored: the
                // relation is linear, so true samples stay close to the base.
                float est = skyAvg / (cur / 100.0f);
                if (est > skyBase[skyGait] * 0.8f && est < skyBase[skyGait] * 1.25f)
                    skyBase[skyGait] += (est - skyBase[skyGait]) * 0.05f;
            }
            speedTimer += dt;
            if (driven) {
                // Snake moves the player himself: the player's own SpeedMult.
                float base = g_baseSpeedMult;
                if (speedTimer > 0.2 && base > 0 && g_speedMult > 0 && std::fabs(g_speedMult - base) > 0.01f) {
                    speedTimer = 0;
                    RequestSpeedMult(base);
                }
            } else if (speedTimer > 0.2 && g_control.speed && !g_speedBroken) {
                speedTimer = 0;
                if (cur < 0) {
                    RequestSpeedMult(-1.0f);  // read the player's own value first
                } else {
                    // Standing still: preset the value for running in this
                    // stance, or the player's own until it is known.
                    bool holdSpeed = false;
                    int st = moving ? state : std::clamp(now.stance, 0, 2) * 4 + 2;
                    int sg = moving ? skyGait : 1;
                    float target = snakeSpeedOf[st];
                    if (moving && target <= 0 && movingTime > 0.5) target = snakeAvg;
                    // Unknown while moving (e.g. during a stance transition,
                    // which reads as idle): keep the current value rather
                    // than jumping back to the player's own speed.
                    if (moving && target <= 0.1f) holdSpeed = true;
                    float want = target > 0.1f ? 100.0f * target / skyBase[sg] : g_baseSpeedMult.load();
                    // Skyrim will not start moving (or turn round) from a
                    // standstill below roughly SpeedMult 20, though it keeps
                    // moving at 15 (the prone crawl), so the idle preset
                    // stays at 35 or more.
                    if (!moving) want = (std::max)(want, 35.0f);
                    want = std::clamp(want, 5.0f, 400.0f);
                    if (!holdSpeed && std::fabs(want - cur) > cur * 0.04f) {
                        RequestSpeedMult(want);
                        sinceSpeedChange = 0;
                    }
                }
            }
            if (g_snake.player_foley && !g_foleyVoices.empty()) {
                SnakeSound sounds[16];
                const char* surface = GroundSurface(ThePlayer());
                uint32_t n = g_snake.player_foley(g_runtime, g_player, surface, 0, sounds, 16);
                for (uint32_t s = 0; s < n; ++s) {
                    PlayFoley(sounds[s]);
                    if (g_control.trace) Log("trace: foley kind %d, %.2f s, %s", sounds[s].kind, sounds[s].frames / 48000.0, surface);
                }
            }
            SnakeShot shots[16];
            uint32_t k = g_snake.player_take_shots(g_player, shots, 16);
            if (g_control.launch) QueueShots(shots, k);
            if (k) {
                SnakeStatus st{};
                g_snake.player_status(g_player, &st);
                PlayShot(st.weapon);
                if (g_control.flash) TriggerFlash();
            }
            if (g_control.dump) {
                g_control.dump = false;
                DumpCamera(ThePlayer());
            }
            if (g_control.probe == 1 && g_tasks) {
                g_control.probe = 0;
                g_tasks->AddTask(new ProbeTask());
            }
            if (g_control.probe == 5 && g_tasks) {
                g_control.probe = 0;
                g_tasks->AddTask(new CameraProbeTask());
            }
            if (g_control.probe == 4) {  // PlayerControls floats (the movement input vector)
                g_control.probe = 0;
                void* pc = g_fire.playerControls && Readable(g_fire.playerControls, 8) ? *g_fire.playerControls : nullptr;
                if (pc && Readable(pc, 0x80)) {
                    std::string row;
                    char b[24];
                    for (size_t off = 0x20; off < 0x80; off += 4) {
                        snprintf(b, sizeof b, " %zX:%.2f", off, At<float>(pc, off));
                        row += b;
                    }
                    Log("probe controls:%s", row.c_str());
                }
            }
            if (g_control.probe == 2 && g_tasks) {  // controller dump, with the measured speed
                g_control.probe = 0;
                char label[64];
                snprintf(label, sizeof label, "at %.2f m/s heading %.3f", speed,
                         ThePlayer() ? At<float>(ThePlayer(), kRefAngle + 8) : 0.0f);
                g_tasks->AddTask(new DumpTask(label));
            }
            // Hands drawn while aiming; sheathed again a second after.
            if (in.aim) sinceAim = 0;
            else sinceAim += dt;
            bool wantHands = g_control.draw != 0 && sinceAim < 1.0;
            if (wantHands != handsDrawn) {
                handsDrawn = wantHands;
                RequestHands(wantHands);
            }
        }
        if (g_snake.retarget_apply(g_retarget, g_player, rot.data(), tr.data(), has.data(), n) != n) {
            Log("snake_retarget_apply failed: %s", g_snake.last_error());
            g_enabled = false;
            continue;
        }
        // Each weapon mesh: in the hands, holstered or hidden.
        int gunPlace[4] = {};
        SnakeVec3 gunPos[4] = {};
        SnakeQuat gunRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};
        uint32_t weapons = (std::min)(g_snake.weapon_count(g_runtime), 4u);
        int magPlace[4] = {};
        SnakeVec3 magPos[4] = {};
        SnakeQuat magRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};
        for (uint32_t i = 0; i < weapons; ++i) {
            gunPlace[i] = g_snake.retarget_weapon_place(g_retarget, g_player, i, &gunPos[i], &gunRot[i]);
            magPlace[i] = g_snake.retarget_magazine_place(g_retarget, g_player, i, &magPos[i], &magRot[i]);
        }
        SnakeStatus raisedStatus{};
        g_snake.player_status(g_player, &raisedStatus);
        {
            std::lock_guard<std::mutex> lock(g_pose.m);
            g_pose.raised = raisedStatus.raised;
            g_pose.reloading = raisedStatus.reloading != 0;
            g_pose.rot = rot;
            g_pose.tr = tr;
            g_pose.has = has;
            memcpy(g_pose.gunPlace, gunPlace, sizeof gunPlace);
            memcpy(g_pose.gunPos, gunPos, sizeof gunPos);
            memcpy(g_pose.gunRot, gunRot, sizeof gunRot);
            memcpy(g_pose.magPlace, magPlace, sizeof magPlace);
            memcpy(g_pose.magPos, magPos, sizeof magPos);
            memcpy(g_pose.magRot, magRot, sizeof magRot);
            ++g_pose.seq;
        }
        UpdateHud();
        logTimer += dt;
        if (logTimer > 2.0) {
            logTimer = 0;
            SnakeStatus st{};
            g_snake.player_status(g_player, &st);
            Log("snake: speed %.2f m/s stance %d gait %d aiming %d ammo %d/%d%s, shots %d, mode %d, focused %d, hook calls %u, hands %d, heading %.3f, snake %.2f / skyrim %.2f m/s, SpeedMult %.1f, run base %.2f",
                speed, st.stance, st.gait, st.aiming, st.ammo, st.reserve, st.reloading ? " (reloading)" : "",
                (int)g_shotsFired, (int)g_mode, (int)focused, g_hookCalls.exchange(0), ThePlayer() ? WeaponStateOf(ThePlayer()) : -1,
                ThePlayer() ? At<float>(ThePlayer(), kRefAngle + 8) : 0.0f, snakeAvg, skyAvg, g_speedMult.load(), skyBase[1]);
        }
    }
}

// ---- per-frame hook (Skyrim job threads) -----------------------------------------

std::mutex g_hookMutex;  // the player's hook state (try-locked, never waited on)
float g_lastPos[3] = {0, 0, 0};
bool g_havePos = false;
void* g_cachedRoot = nullptr;
std::atomic<bool> g_resolveNodes{false};  // re-find the retarget bones (finger bones were attached)
std::vector<SnakeQuat> g_hookRot;
std::vector<SnakeVec3> g_hookTr;
std::vector<uint8_t> g_hookHas;
uint64_t g_appliedSeq = 0;
int g_hookFrames = 0;
// Per weapon mesh: 0 hidden, 1 in the hands, 2 holstered (spine-relative).
int g_hookGunPlace[4] = {};
SnakeVec3 g_hookGunPos[4] = {};
SnakeQuat g_hookGunRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};
int g_hookMagPlace[4] = {};
SnakeVec3 g_hookMagPos[4] = {};
SnakeQuat g_hookMagRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};
float g_hookRaised = 0;
bool g_hookReloading = false;

// ---- Snake's gun model ---------------------------------------------------------
//
// The weapon mesh written by `mgsv skyrim-export` (Data\meshes\snake\*.nif)
// is loaded with BSModelDB::Demand and attached under the right hand on the
// main thread; the hook then places it every frame from Snake's grip or his
// holster (snake_retarget_weapon_place). It stays parented to the hand; a
// holstered placement (relative to the spine) is converted through the two
// bones' world transforms.

constexpr size_t kVtAttachChild = 0x35;  // NiNode::AttachChild(NiAVObject*, bool)
const char* const kRightHand = "NPC R Hand [RHnd]";

struct ModelArgs {  // BSModelDB::DBTraits::ArgsType
    uint32_t lodMult = 0;
    uint32_t texLoadLevel = 3;
    bool unk8 = true, unk9 = false, unkA = true, postProcess = true;
};
using DemandFn = uint32_t (*)(const char* path, void** out, const ModelArgs* args);

// One mesh per weapon (weapon index order) and per view: the third-person
// skeleton (view 0) and the first-person one (view 1). Skyrim caches
// models by path, so the first-person instance is a copy of the mesh under
// its own name (`<mesh>_1p.nif`, written by the export).
constexpr int kMaxGuns = 4;
struct GunSlot {
    std::atomic<void*> node[2] = {nullptr, nullptr};    // loaded model root, per view
    std::atomic<void*> parent[2] = {nullptr, nullptr};  // hand node it is attached to
    bool failed[2] = {false, false};                    // no mesh (run `mgsv skyrim-export`)
};
GunSlot g_guns[kMaxGuns];
// Each weapon's magazine: MGSV keeps it a separate model on the gun's
// CNP_AMMO, and it comes out in Snake's left hand during a reload. Attached
// under the same hand as the guns and placed the same way.
GunSlot g_mags[kMaxGuns];
std::atomic<int> g_gunCount{0};  // set once every mesh has been tried
// The muzzle flash is a separate mesh beside the gun (inside the gun's node
// its additive shapes made Skyrim draw the whole gun unlit).
const char* const kFlashMesh[2] = {"snake\\muzzle_flash.nif", "snake\\muzzle_flash_1p.nif"};
std::atomic<void*> g_flashNode[2] = {nullptr, nullptr};
std::atomic<void*> g_flashParent[2] = {nullptr, nullptr};
std::vector<SnakeVec3> g_muzzles;  // per weapon, in its mesh frame
std::atomic<bool> g_gunQueued{false};

uint32_t SafeDemand(const char* path, void** out) {
    __try {
        ModelArgs args;
        return reinterpret_cast<DemandFn>(g_fire.modelDemand)(path, out, &args);
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return 0xFFFFFFFF;
    }
}

bool SafeAttach(void* parent, void* child) {
    __try {
        using Fn = void (*)(void*, void*, bool);
        reinterpret_cast<Fn>(Vtable(parent)[kVtAttachChild])(parent, child, false);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

void SetHidden(void* node, bool hidden) {
    uint32_t& flags = At<uint32_t>(node, 0xF4);
    flags = hidden ? (flags | 1u) : (flags & ~1u);
}

// Mesh path for a view: "snake\\x.nif" -> "snake\\x_1p.nif" for first person.
std::string ViewMesh(const char* mesh, int view) {
    std::string m = mesh ? mesh : "";
    if (view == 1 && m.size() > 4) m.insert(m.size() - 4, "_1p");
    return m;
}

class AttachGunTask : public TaskDelegate {
public:
    void Run() override {
        g_gunQueued = false;
        void* player = ThePlayer();
        if (!player || !g_fire.modelDemand) return;
        int count = (std::min)((int)g_snake.weapon_count(g_runtime), kMaxGuns);
        if (g_muzzles.empty()) {
            for (int i = 0; i < count; ++i) {
                SnakeVec3 m{};
                g_snake.weapon_muzzle(g_runtime, i, &m);
                g_muzzles.push_back(m);
            }
        }
        for (int view = 0; view < 2; ++view) {
            void* root = Get3D(player, view == 1);
            void* hand = root ? FindNode(root, kRightHand) : nullptr;
            if (!hand) continue;
            for (int i = 0; i < 2 * count; ++i) {
                bool magazine = i >= count;
                int w = magazine ? i - count : i;
                GunSlot& g = magazine ? g_mags[w] : g_guns[w];
                const char* source = magazine ? g_snake.weapon_magazine_mesh(g_runtime, w) : g_snake.weapon_mesh(g_runtime, w);
                if (!source) g.failed[view] = true;
                if (!g.node[view] && !g.failed[view]) {
                    std::string mesh = ViewMesh(source, view);
                    void* node = nullptr;
                    uint32_t err = SafeDemand(mesh.c_str(), &node);
                    Log("gun model %s: error %u, node %p", mesh.c_str(), err, node);
                    if (err != 0 || !node) {
                        g.failed[view] = true;
                        continue;
                    }
                    SetHidden(node, true);
                    g.node[view] = node;
                }
                void* gun = g.node[view];
                if (gun && g.parent[view] != hand) {
                    if (SafeAttach(hand, gun)) {
                        g.parent[view] = hand;
                        Log("%s model %d attached to the %s-person %s (%p)", magazine ? "magazine" : "gun", w,
                            view ? "first" : "third", kRightHand, hand);
                    } else {
                        g.failed[view] = true;
                        Log("attaching %s model %d faulted", magazine ? "magazine" : "gun", w);
                    }
                }
            }
            if (!g_flashNode[view]) {
                void* node = nullptr;
                uint32_t err = SafeDemand(kFlashMesh[view], &node);
                Log("muzzle flash model %s: error %u, node %p", kFlashMesh[view], err, node);
                if (err == 0 && node) g_flashNode[view] = node;
            }
            void* flash = g_flashNode[view];
            if (flash && g_flashParent[view] != hand && SafeAttach(hand, flash)) {
                g_flashParent[view] = hand;
                SetHidden(flash, true);
            }
        }
        g_gunCount = count;
    }
    void Dispose() override { delete this; }
};

// Local transform (rotation matrix `r`, position `lp`) under `hand` for a
// placement from snake_retarget_weapon_place: hand-relative as is, or
// spine-relative converted through the bones' world transforms.
void HandLocal(int place, const SnakeVec3& pos, const SnakeQuat& rot, void* hand, void* spine, float* r, float* lp) {
    if (place == 1) {
        QuatToMat(rot, r);
        lp[0] = pos.x;
        lp[1] = pos.y;
        lp[2] = pos.z;
        return;
    }
    float rs[9], rw[9], pw[3], d[3];
    QuatToMat(rot, rs);
    const float* ms = &At<float>(spine, kWorldRot);
    const float* mh = &At<float>(hand, kWorldRot);
    const float* ps = &At<float>(spine, kWorldPos);
    const float* ph = &At<float>(hand, kWorldPos);
    float ss = At<float>(spine, kWorldScale), sh = At<float>(hand, kWorldScale);
    float pl[3] = {pos.x * ss, pos.y * ss, pos.z * ss};
    MatMul(ms, rs, rw);
    MatVec(ms, pl, pw);
    for (int k = 0; k < 3; ++k) d[k] = ps[k] + pw[k] - ph[k];
    for (int i = 0; i < 3; ++i) {
        for (int j = 0; j < 3; ++j)  // r = mh^T rw
            r[i * 3 + j] = mh[0 * 3 + i] * rw[0 * 3 + j] + mh[1 * 3 + i] * rw[1 * 3 + j] + mh[2 * 3 + i] * rw[2 * 3 + j];
        lp[i] = (mh[0 * 3 + i] * d[0] + mh[1 * 3 + i] * d[1] + mh[2 * 3 + i] * d[2]) / (sh > 0 ? sh : 1.0f);
    }
}

// Place (or hide) every gun of a view under `hand`; queue the attach when
// needed. Local transforms only: Skyrim derives the world transforms (and
// the renderer's per-object state) in its next update. Writing a gun's world
// transform here, as for the bones, made the renderer draw it as a flat
// fog-coloured silhouette on some frames. In first person a holstered gun
// is not shown.
void ShowHolsterContents();  // below, after the body slots

void PlaceGun(int view, void* hand, void* spine) {
    if (!hand) return;
    if (view == 0) ShowHolsterContents();
    int count = g_gunCount;
    bool attached = count > 0;
    for (int i = 0; i < count; ++i) {
        if (!g_guns[i].failed[view] && g_guns[i].parent[view] != hand) attached = false;
        if (!g_mags[i].failed[view] && g_mags[i].parent[view] != hand) attached = false;
    }
    if (!attached) {
        if (g_tasks && !g_gunQueued.exchange(true)) g_tasks->AddTask(new AttachGunTask());
        return;
    }
    int inHand = -1;
    for (int i = 0; i < count; ++i) {
        void* gun = g_guns[i].node[view];
        if (!gun) continue;
        int place = g_hookGunPlace[i];
        if (place == 0 || (place == 2 && (!spine || view == 1))) {
            SetHidden(gun, true);
            continue;
        }
        SetHidden(gun, false);
        HandLocal(place, g_hookGunPos[i], g_hookGunRot[i], hand, spine, &At<float>(gun, kLocalRot),
                  &At<float>(gun, kLocalPos));
        if (place == 1) inHand = i;
    }
    for (int i = 0; i < count; ++i) {
        void* mag = g_mags[i].node[view];
        if (!mag) continue;
        int place = g_hookMagPlace[i];
        if (place == 0 || (place == 2 && (!spine || view == 1))) {
            SetHidden(mag, true);
            continue;
        }
        SetHidden(mag, false);
        HandLocal(place, g_hookMagPos[i], g_hookMagRot[i], hand, spine, &At<float>(mag, kLocalRot),
                  &At<float>(mag, kLocalPos));
    }
    // Flash: same rotation as the gun in the hands, at its muzzle; one
    // random cell.
    void* flash = g_flashParent[view] == hand ? static_cast<void*>(g_flashNode[view]) : nullptr;
    if (!flash) return;
    bool flashing = inHand >= 0 && inHand < (int)g_muzzles.size() && NowMs() < g_flashUntil;
    SetHidden(flash, !flashing);
    if (!flashing) return;
    void* gun = g_guns[inHand].node[view];
    const float* r = &At<float>(gun, kLocalRot);
    const float* lp = &At<float>(gun, kLocalPos);
    const SnakeVec3& m = g_muzzles[inHand];
    memcpy(&At<float>(flash, kLocalRot), r, 9 * sizeof(float));
    float* fp = &At<float>(flash, kLocalPos);
    float mv[3] = {m.x, m.y, m.z}, rm[3];
    MatVec(r, mv, rm);
    for (int k = 0; k < 3; ++k) fp[k] = lp[k] + rm[k];
    int cell = g_flashCell;
    ForEachChild(flash, [&](void* c) {
        const char* name = Name(c);
        if (name && strncmp(name, "muzzle_flash:", 13) == 0) SetHidden(c, name[13] - '0' != cell);
    });
}

// Skyrim's own weapon and shield attach under the WEAPON and SHIELD nodes
// (drawn when Snake aims); those are hidden while Snake mode is on, in both
// the third- and first-person skeletons. NiAVObject::flags is at +0xF4,
// bit 0 = hidden; each node's own bit is restored afterwards.
constexpr size_t kAvFlags = 0xF4;
const char* const kWeaponNodeNames[] = {"WEAPON", "SHIELD"};
struct WeaponNodes {
    void* root = nullptr;
    void* nodes[2] = {};
    uint32_t wasHidden[2] = {};
    bool hidden = false;
};
WeaponNodes g_weaponNodes[2];  // third person, first person

void SetWeaponsHidden(void* player, bool hide) {
    for (int fp = 0; fp < 2; ++fp) {
        WeaponNodes& w = g_weaponNodes[fp];
        void* root = Get3D(player, fp == 1);
        if (root != w.root) {  // new 3D: the old nodes are gone
            w = WeaponNodes{};
            w.root = root;
            for (int i = 0; i < 2 && root; ++i) w.nodes[i] = FindNode(root, kWeaponNodeNames[i]);
        }
        // Skyrim clears the flag when it attaches a drawn weapon, so it is
        // set again every frame.
        if (!root || (!hide && !w.hidden)) continue;
        for (int i = 0; i < 2; ++i) {
            if (!w.nodes[i]) continue;
            uint32_t& flags = At<uint32_t>(w.nodes[i], kAvFlags);
            if (hide) {
                if (!w.hidden) w.wasHidden[i] = flags & 1u;
                flags |= 1u;
            } else {
                flags = (flags & ~1u) | w.wasHidden[i];
            }
        }
        w.hidden = hide;
    }
}

bool SafeSetWeaponsHidden(void* player, bool hide) {
    __try {
        SetWeaponsHidden(player, hide);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// ---- Snake's body ----------------------------------------------------------------
//
// `mgsv skyrim-export` writes Snake's MGSV body, face and bionic arm as one
// skinned mesh (Data\\meshes\\snake\\snake_body.nif) bound to Skyrim's core
// skeleton bones in Snake's rest pose. A main-thread task loads it, points
// each shape's skin instance at the live skeleton's bones (by name) and
// attaches it to the player's 3D root. While Snake mode is on it is shown
// and the root's other children (Skyrim's body, armour, head) are hidden.
// The first-person view does the same with Snake's arms
// (snake_arms_1p.nif, bound to the first-person arm and finger bones).
// NiSkinInstance (geometry + 0x130): rootParent + 0x20, bones + 0x28,
// boneWorldTransforms + 0x30, numMatrices + 0x3C.

constexpr size_t kSkinRootParent = 0x20, kSkinBoneWorlds = 0x30;  // the rest: kGeomSkinInstance, kSkinBones, kSkinBoneCount
struct BodySlot {
    const char* mesh;
    std::atomic<void*> node{nullptr};
    std::atomic<void*> parent{nullptr};  // 3D root it is attached to
    std::atomic<bool> queued{false};
    bool failed = false;
    std::vector<void*> hidden;  // Skyrim nodes we hid (restored when Snake mode ends)
    void* alwaysDrawn = nullptr;  // the node last marked always drawn
};
BodySlot g_bodies[2] = {{"snake\\snake_body.nif"}, {"snake\\snake_arms_1p.nif"}};  // per view (0 third person, 1 first)

// The pistol's thigh holster is closed: MGSV models the holstered gun into
// Snake's body (its grip shows at the back of the holster) and shows that
// part only while the gun is in there. Per weapon, those shapes of the
// third-person body (named by snake_weapon_holster_shape), found once per
// body node.
constexpr int kMaxHolsterShapes = 4;
void* g_holsterShapes[4][kMaxHolsterShapes] = {};
void* g_holsterShapesBody = nullptr;

void ShowHolsterContents() {
    void* body = g_bodies[0].node;
    if (!body || !g_snake.weapon_holster_shape) return;
    int count = g_gunCount;
    if (body != g_holsterShapesBody) {
        g_holsterShapesBody = body;
        for (int i = 0; i < count && i < 4; ++i)
            for (int k = 0; k < kMaxHolsterShapes; ++k) {
                const char* name = g_snake.weapon_holster_shape(g_runtime, i, k);
                g_holsterShapes[i][k] = name ? FindNode(body, name) : nullptr;
                if (name) Log("holster contents of weapon %d: %s %s", i, name, g_holsterShapes[i][k] ? "found" : "missing");
            }
    }
    for (int i = 0; i < count && i < 4; ++i)
        for (void* shape : g_holsterShapes[i])
            if (shape) SetHidden(shape, g_hookGunPlace[i] != 0);
}

// Fingers: the in-game third-person skeleton has no finger bones, so the
// export ships each hand's finger bones as a node tree (rest pose relative
// to the hand) and they are attached under the live hands before the body's
// skin is bound to them. Snake's pose then reaches them like any other bone
// (the retarget output has them), so his hands grip as in MGSV.
struct FingerSlot {
    const char* mesh;
    const char* hand;
    std::atomic<void*> node{nullptr};    // the tree's root ("SnakeFingers L/R")
    std::atomic<void*> parent{nullptr};  // live hand it is attached to
};
// The strap hanging at Snake's back works the same way: its four bones
// (simulated by snake.dll, as MGSV simulates them) hang under the pelvis.
FingerSlot g_fingers[3] = {{"snake\\snake_fingers_l.nif", "NPC L Hand [LHnd]"},
                           {"snake\\snake_fingers_r.nif", "NPC R Hand [RHnd]"},
                           {"snake\\snake_strap.nif", "NPC Pelvis [Pelv]"}};

// Main thread: attach both finger trees under `skeleton`'s hands.
void AttachFingers(void* skeleton) {
    for (FingerSlot& f : g_fingers) {
        void* hand = FindNode(skeleton, f.hand);
        if (!hand || f.parent == hand) continue;
        if (!f.node) {
            void* node = nullptr;
            uint32_t err = SafeDemand(f.mesh, &node);
            if (err != 0 || !node) {
                Log("fingers %s: error %u (run mgsv skyrim-export)", f.mesh, err);
                continue;
            }
            f.node = node;
        }
        if (!SafeAttach(hand, f.node)) {
            Log("fingers %s: attaching faulted", f.mesh);
            continue;
        }
        f.parent = hand;
        Log("fingers %s attached under %s", f.mesh, f.hand);
    }
    g_resolveNodes = true;
}

bool IsFingerRoot(void* node) {
    if (!node) return false;
    for (FingerSlot& f : g_fingers)
        if (node == f.node) return true;
    return false;
}

// Point every skin instance under `node` at the bones of `skeleton`.
int RebindSkin(void* node, void* skeleton, void* rootParent) {
    int rebound = 0;
    if (void* geom = AsGeometry(node)) {
        void* skin = At<void*>(geom, kGeomSkinInstance);
        if (skin && Readable(skin, 0x40)) {
            // numMatrices is only filled in once the shape renders; the
            // skin data (+0x10) holds the bone count at +0x58 (after its
            // root-to-skin transform and bone data pointer).
            uint32_t n = At<uint32_t>(skin, kSkinBoneCount);
            void* data = At<void*>(skin, 0x10);
            if (!n && data && Readable(data, 0x60)) n = At<uint32_t>(data, 0x58);
            if (n > 128) {
                Log("body: implausible bone count %u; skin left as loaded", n);
                n = 0;
            }
            void** bones = At<void**>(skin, kSkinBones);
            void** worlds = At<void**>(skin, kSkinBoneWorlds);
            for (uint32_t i = 0; i < n && bones && worlds; ++i) {
                const char* name = bones[i] ? Name(bones[i]) : nullptr;
                void* live = name ? FindNode(skeleton, name) : nullptr;
                if (!live) {
                    Log("body: no live bone %s", name ? name : "?");
                    continue;
                }
                bones[i] = live;
                worlds[i] = &At<char>(live, kWorldRot);  // NiTransform: rotation, translation, scale
                ++rebound;
            }
            At<void*>(skin, kSkinRootParent) = rootParent;
        }
    }
    if (AsNode(node)) ForEachChild(node, [&](void* c) { rebound += RebindSkin(c, skeleton, rootParent); });
    return rebound;
}

class BodyTask : public TaskDelegate {
public:
    explicit BodyTask(int view) : view_(view) {}
    void Run() override {
        BodySlot& b = g_bodies[view_];
        b.queued = false;
        void* player = ThePlayer();
        void* root = player ? Get3D(player, view_ == 1) : nullptr;
        void* skeleton = root ? FindNode(root, "NPC Root [Root]") : nullptr;
        if (!skeleton || !g_fire.modelDemand) return;
        if (b.parent == root) return;
        if (!b.node) {
            void* node = nullptr;
            uint32_t err = SafeDemand(b.mesh, &node);
            Log("body model %s: error %u, node %p", b.mesh, err, node);
            if (err != 0 || !node) {
                b.failed = true;
                return;
            }
            SetHidden(node, true);
            b.node = node;
        }
        void* body = b.node;
        if (view_ == 0) AttachFingers(skeleton);
        int rebound = RebindSkin(body, skeleton, root);
        if (!SafeAttach(root, body)) {
            b.failed = true;
            Log("attaching %s faulted", b.mesh);
            return;
        }
        b.parent = root;
        b.hidden.clear();
        Log("%s attached to %s (%p): %d skin bones rebound", b.mesh, Name(root) ? Name(root) : "?", root, rebound);
        ForEachChild(root, [&](void* c) { Log("  root child %s", Name(c) ? Name(c) : "?"); });
    }
    void Dispose() override { delete this; }

private:
    int view_;
};

// Show Snake's body (view 0) or first-person arms (view 1) and hide the
// player's own parts under `root`, or the reverse.
void ShowBody(int view, void* root, bool snake) {
    BodySlot& b = g_bodies[view];
    void* body = b.node;
    if (!root || b.failed) return;
    if (b.parent != root) {
        if (snake && g_tasks && !b.queued.exchange(true)) g_tasks->AddTask(new BodyTask(view));
        return;
    }
    if (!body) return;
    SetHidden(body, !snake);
    if (view == 1 && b.alwaysDrawn != body) {
        // The arms' bounds stay where the mesh was loaded: held up in front
        // of the camera (the pistol's view model) they were culled whenever
        // the view looked level or up. NiAVObject kAlwaysDraw (bit 11).
        std::function<void(void*)> mark = [&](void* n) {
            At<uint32_t>(n, kAvFlags) |= 1u << 11;
            if (void* node = AsNode(n)) ForEachChild(node, mark);
        };
        mark(body);
        b.alwaysDrawn = body;
    }
    if (snake) {
        // Skyrim re-shows parts it (re)attaches, so hide every frame.
        ForEachChild(root, [&](void* c) {
            if (c == body) return;
            const char* name = Name(c);
            if (name && (strcmp(name, "NPC") == 0 || strcmp(name, "NPC Root [Root]") == 0)) return;
            if (!(At<uint32_t>(c, 0xF4) & 1u)) {
                SetHidden(c, true);
                b.hidden.push_back(c);
            }
        });
    } else if (!b.hidden.empty()) {
        for (void* c : b.hidden) SetHidden(c, false);
        b.hidden.clear();
    }
}

// Write Snake's local rotations (and COM translation) to `nodes` (retarget
// order; null entries skipped), then their world transforms parent-first.
void WriteWorlds(const std::vector<void*>& nodes, size_t n);

void WritePose(const std::vector<void*>& nodes, size_t n, bool world) {
    for (size_t i = 0; i < n; ++i) {
        void* node = nodes[i];
        if (!node) continue;
        QuatToMat(g_hookRot[i], &At<float>(node, kLocalRot));
        if (g_hookHas[i]) {
            float* lp = &At<float>(node, kLocalPos);
            lp[0] = g_hookTr[i].x;
            lp[1] = g_hookTr[i].y;
            lp[2] = g_hookTr[i].z;
        }
    }
    if (world) WriteWorlds(nodes, n);
}

void WriteWorlds(const std::vector<void*>& nodes, size_t n) {
    for (size_t i = 0; i < n; ++i) {
        void* node = nodes[i];
        if (!node) continue;
        void* parent = At<void*>(node, kParent);
        if (!parent) continue;
        if (IsFingerRoot(parent)) {
            // The finger tree's root (identity under the hand, not a
            // retarget bone): bring its world up to the hand's first.
            void* hand = At<void*>(parent, kParent);
            if (hand) {
                memcpy(&At<float>(parent, kWorldRot), &At<float>(hand, kWorldRot), 9 * sizeof(float));
                memcpy(&At<float>(parent, kWorldPos), &At<float>(hand, kWorldPos), 3 * sizeof(float));
                At<float>(parent, kWorldScale) = At<float>(hand, kWorldScale);
            }
        }
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
}

// First person: only the arm chains (clavicles down to the fingers) follow
// Snake; the spine, neck and head stay Skyrim's so the first-person camera
// is not moved. Skyrim poses its first-person spine very differently from
// the third-person one, so each clavicle gets the world orientation of its
// third-person counterpart (where Snake already aims along the camera) and
// the rest of the chain keeps Snake's local rotations. Unlike the
// third-person skeleton this one has fingers.
void* g_cachedRoot1p = nullptr;
std::vector<void*> g_nodes1p;
std::vector<std::array<float, 3>> g_rest1p;  // clavicle local positions as loaded
constexpr float kFirstPersonPistolDrop = 10.0f;  // Skyrim units (~14 cm)

struct CameraNode {
    void* root = nullptr;
    void* node = nullptr;
    float baseZ = 0;
};
CameraNode g_cameraNodes[2];  // third person, first person

// First person, the pistol raised: the aim poses hold it two-handed at eye
// level for the over-the-shoulder camera, so in first person it covered the
// crosshair, and the old fix (shoulders 10 units down) put it off the
// bottom of the screen. Instead both shoulders shift together each frame so
// the gun lands at a fixed place from the first-person camera node, in the
// camera's frame (right, forward, up; Skyrim units, tuned by eye): just
// right of and below the crosshair, sights in view, at any pitch. The aim
// already points it along the view, so it is a shift only (turning the
// arms as well made them flicker out of view). Blended with how far the
// gun is raised; lowered, the old 10 units down. During a reload the shift
// it started with is kept (relative to the camera), so the reload plays out
// in view rather than the gun staying put while the shoulders swing.
constexpr float kPistolView[3] = {1.0f, 40.0f, -2.5f};
constexpr float kPistolShiftMax = 15.0f;  // Skyrim units (aimed it is a few)

// The camera's world rotation (columns: right, forward, up). (Its position
// is not in the first-person skeleton's space while the hook runs; its
// rotation is.)
bool CameraRotation(float* rot) {
    if (!g_fire.camera) return false;
    void* cam = *g_fire.camera;
    if (!cam || !Readable(cam, kCameraRoot + 8)) return false;
    void* root = At<void*>(cam, kCameraRoot);
    if (!root || !Readable(root, kWorldScale + 4)) return false;
    memcpy(rot, &At<float>(root, kWorldRot), 9 * sizeof(float));
    float fy = rot[1] * rot[1] + rot[4] * rot[4] + rot[7] * rot[7];
    return fy > 0.5f && fy < 2.0f;
}

bool IsArmBone(const std::string& n) {
    for (const char* part : {"Clavicle", "UpperArm", "Upperarm", "Forearm", "Hand", "Finger"})
        if (n.find(part) != std::string::npos) return true;
    return false;
}

void PlaceCameras(void* player, float drop);
void FollowTurn(void* player);

void ApplySnake(void* player, float delta) {
    void* root = Get3D(player, false);
    if (!root) return;
    bool newRoot = root != g_cachedRoot;
    if (newRoot || g_resolveNodes.exchange(false)) {
        g_nodes.clear();
        int missing = 0;
        for (auto& n : g_boneNames) {
            g_nodes.push_back(FindNode(root, n));
            missing += g_nodes.back() == nullptr;
        }
        g_cachedRoot = root;
        Log("player 3D %p (%s): found %zu of %zu retarget bones in the node tree", root, Name(root) ? Name(root) : "?",
            g_nodes.size() - missing, g_nodes.size());
        if (missing) {
            std::unordered_map<std::string, void*> skinBones;
            CollectSkinBones(root, skinBones);
            int recovered = 0;
            for (size_t i = 0; i < g_nodes.size(); ++i) {
                if (g_nodes[i]) continue;
                auto it = skinBones.find(g_boneNames[i]);
                if (it == skinBones.end()) continue;
                g_nodes[i] = it->second;
                ++recovered;
                void* parent = At<void*>(it->second, kParent);
                if (recovered <= 3)
                    Log("  via skin: %s (parent %s)", g_boneNames[i].c_str(),
                        parent && Readable(parent, 0x18) && Name(parent) ? Name(parent) : "none");
            }
            // In third person Skyrim only keeps the bones its meshes use: fingers,
            // toes and forearm twists exist in the first-person skeleton only.
            Log("recovered %d bones via skinned geometry; %d not in the third-person skeleton (first-person only)",
                recovered, missing - recovered);
        }
        // Once per skeleton, before Snake's pose is written to it (a
        // re-resolve for the attached fingers keeps the verdict).
        if (newRoot) {
            g_layoutRetries = 0;
            CheckLayout();
        }
        g_hookFrames = 0;
    }
    // A failed check is retried every half second (a skeleton just loaded
    // has stale world transforms), up to 20 times.
    static int layoutWait = 0;
    if (!g_layoutOk && !g_nodes.empty() && g_layoutRetries < 20 && ++layoutWait >= 30) {
        layoutWait = 0;
        CheckLayout();
    }

    // Player speed for Snake's gait.
    float* pos = &At<float>(player, kRefPosition);
    if (g_havePos && delta > 0) {
        float dx = pos[0] - g_lastPos[0], dy = pos[1] - g_lastPos[1];
        g_speed = std::sqrt(dx * dx + dy * dy) / delta / 70.0f;
        float h = At<float>(player, kRefAngle + 8);
        g_moveForward = (dx * std::sin(h) + dy * std::cos(h)) / delta / 70.0f;
        g_moveRight = (dx * std::cos(h) - dy * std::sin(h)) / delta / 70.0f;
    }
    {
        // Skyrim's sneak flag (bit 9) follows Snake's stance: written here
        // only until Skyrim's own toggle is available (see SneakSyncTask).
        int sneak = g_wantSneak;
        if (sneak >= 0 && (!g_haveSneakTemplate || g_sneakToggleBroken)) {
            uint32_t& state = At<uint32_t>(player, kActorState1);
            state = (state & ~(1u << 9)) | (static_cast<uint32_t>(sneak) << 9);
        }
        PlaceCameras(player, g_cameraDrop);
        g_actorFlags = At<uint32_t>(player, kActorState1);
        // Aim pitch follows the camera (positive looks down), else the actor.
        float cam[3], dir[3];
        bool haveCamera = CameraAim(cam, dir);
        g_pitch = haveCamera ? -std::asin((std::max)(-1.0f, (std::min)(1.0f, dir[2]))) : At<float>(player, kRefAngle);
        if (haveCamera && dir[0] * dir[0] + dir[1] * dir[1] > 1e-4f) {
            g_cameraYaw = std::atan2(dir[0], dir[1]);
            g_cameraYawValid = true;
        }
    }
    memcpy(g_lastPos, pos, sizeof g_lastPos);
    g_havePos = true;

    int mode = g_mode;
    if (mode < 1 || !g_layoutOk) return;
    // Latest pose from the Snake thread, if it is not mid-write.
    {
        std::unique_lock<std::mutex> lock(g_pose.m, std::try_to_lock);
        if (lock.owns_lock() && g_pose.seq != g_appliedSeq) {
            g_hookRot = g_pose.rot;
            g_hookTr = g_pose.tr;
            g_hookHas = g_pose.has;
            memcpy(g_hookGunPlace, g_pose.gunPlace, sizeof g_hookGunPlace);
            memcpy(g_hookGunPos, g_pose.gunPos, sizeof g_hookGunPos);
            memcpy(g_hookGunRot, g_pose.gunRot, sizeof g_hookGunRot);
            memcpy(g_hookMagPlace, g_pose.magPlace, sizeof g_hookMagPlace);
            memcpy(g_hookMagPos, g_pose.magPos, sizeof g_hookMagPos);
            memcpy(g_hookMagRot, g_pose.magRot, sizeof g_hookMagRot);
            g_hookRaised = g_pose.raised;
            g_hookReloading = g_pose.reloading;
            g_appliedSeq = g_pose.seq;
        }
    }
    size_t n = (std::min)(g_nodes.size(), g_hookRot.size());
    if (mode >= 2) FollowTurn(player);
    WritePose(g_nodes, n, mode >= 2);
    if (mode >= 2) {
        void* hand = nullptr;
        void* spine = nullptr;
        for (size_t i = 0; i < n; ++i) {
            if (g_boneNames[i] == kRightHand) hand = g_nodes[i];
            if (g_boneNames[i] == "NPC Spine2 [Spn2]") spine = g_nodes[i];
        }
        PlaceGun(0, hand, spine);
        ShowBody(0, Get3D(player, false), g_control.body != 0);
        // First-person arms and guns.
        if (void* root1p = Get3D(player, true)) {
            if (root1p != g_cachedRoot1p) {
                g_nodes1p.clear();
                int found = 0;
                for (auto& name : g_boneNames) {
                    g_nodes1p.push_back(IsArmBone(name) ? FindNode(root1p, name) : nullptr);
                    found += g_nodes1p.back() != nullptr;
                }
                g_cachedRoot1p = root1p;
                g_rest1p.assign(g_nodes1p.size(), {0, 0, 0});
                for (size_t i = 0; i < g_nodes1p.size(); ++i)
                    if (g_nodes1p[i]) memcpy(g_rest1p[i].data(), &At<float>(g_nodes1p[i], kLocalPos), 3 * sizeof(float));
                Log("first-person 3D %p: %d arm bones", root1p, found);
            }
            size_t n1 = (std::min)(g_nodes1p.size(), g_hookRot.size());
            WritePose(g_nodes1p, n1, false);
            float cameraRot[9];
            bool cameraOk = CameraRotation(cameraRot);
            float raised = (std::min)((std::max)(g_hookRaised, 0.0f), 1.0f);
            bool viewModel = g_control.viewModel && cameraOk && g_cameraNodes[1].node;
            for (size_t i = 0; i < n1 && i < g_nodes.size(); ++i) {
                void* node = g_nodes1p[i];
                void* third = g_nodes[i];
                if (!node || !third || g_boneNames[i].find("Clavicle") == std::string::npos) continue;
                void* parent = At<void*>(node, kParent);
                if (!parent) continue;
                const float* pw = &At<float>(parent, kWorldRot);
                const float* tw = &At<float>(third, kWorldRot);
                float* r = &At<float>(node, kLocalRot);
                for (int a = 0; a < 3; ++a)
                    for (int b = 0; b < 3; ++b)  // r = pw^T tw
                        r[a * 3 + b] = pw[0 * 3 + a] * tw[0 * 3 + b] + pw[1 * 3 + a] * tw[1 * 3 + b] + pw[2 * 3 + a] * tw[2 * 3 + b];
                // Two-handed pistol aim holds the gun at eye level, so in
                // first person the hands would cover the crosshair: lowered,
                // the shoulders drop; raised, they shift (see kPistolView).
                float drop[3] = {0, 0, g_cameraDrop}, local[3];
                if (g_hookGunPlace[1] == 1)
                    drop[2] -= kFirstPersonPistolDrop * (viewModel ? 1.0f - raised : 1.0f);
                for (float& d : drop) d /= At<float>(parent, kWorldScale);
                float* lp = &At<float>(node, kLocalPos);
                for (int a = 0; a < 3; ++a)  // local = pw^T drop
                    local[a] = pw[0 * 3 + a] * drop[0] + pw[1 * 3 + a] * drop[1] + pw[2 * 3 + a] * drop[2];
                if (i < g_rest1p.size())
                    for (int a = 0; a < 3; ++a) lp[a] = g_rest1p[i][a] + local[a];
            }
            WriteWorlds(g_nodes1p, n1);
            void* hand1p = nullptr;
            for (size_t i = 0; i < n1; ++i)
                if (g_boneNames[i] == kRightHand) hand1p = g_nodes1p[i];
            if (viewModel && hand1p && g_hookGunPlace[1] == 1) {
                // The shift (world) that puts the gun at kPistolView from
                // the camera node, or during a reload the camera-relative
                // one it started with.
                static float held[3];  // camera frame
                float delta[3];
                if (g_hookReloading) {
                    MatVec(cameraRot, held, delta);
                } else {
                    const float* hr = &At<float>(hand1p, kWorldRot);
                    const float* hp = &At<float>(hand1p, kWorldPos);
                    float hs = At<float>(hand1p, kWorldScale);
                    float off[3] = {g_hookGunPos[1].x * hs, g_hookGunPos[1].y * hs, g_hookGunPos[1].z * hs}, gp[3], want[3];
                    MatVec(hr, off, gp);
                    float view[3] = {std::isnan(g_control.viewX) ? kPistolView[0] : g_control.viewX,
                                     std::isnan(g_control.viewY) ? kPistolView[1] : g_control.viewY,
                                     std::isnan(g_control.viewZ) ? kPistolView[2] : g_control.viewZ};
                    MatVec(cameraRot, view, want);
                    const float* ep = &At<float>(g_cameraNodes[1].node, kWorldPos);
                    // Eased in, and at most kPistolShiftMax: mid-draw the gun
                    // is still at the holster, and the full shift would
                    // sweep the arms across the view.
                    float len = 0;
                    for (int k = 0; k < 3; ++k) {
                        delta[k] = ep[k] + want[k] - (hp[k] + gp[k]);
                        len += delta[k] * delta[k];
                    }
                    len = std::sqrt(len);
                    float scale = raised * raised * (len > kPistolShiftMax ? kPistolShiftMax / len : 1.0f);
                    for (float& d : delta) d *= scale;
                    for (int a = 0; a < 3; ++a)  // held = cameraRot^T delta
                        held[a] = cameraRot[0 * 3 + a] * delta[0] + cameraRot[1 * 3 + a] * delta[1] + cameraRot[2 * 3 + a] * delta[2];
                }
                for (size_t i = 0; i < n1; ++i) {
                    void* node = g_nodes1p[i];
                    if (!node || g_boneNames[i].find("Clavicle") == std::string::npos) continue;
                    void* parent = At<void*>(node, kParent);
                    if (!parent) continue;
                    const float* pw = &At<float>(parent, kWorldRot);
                    float ps = At<float>(parent, kWorldScale);
                    float* lp = &At<float>(node, kLocalPos);
                    for (int a = 0; a < 3; ++a)  // local += pw^T delta
                        lp[a] += (pw[0 * 3 + a] * delta[0] + pw[1 * 3 + a] * delta[1] + pw[2 * 3 + a] * delta[2]) / (ps > 0 ? ps : 1.0f);
                }
                WriteWorlds(g_nodes1p, n1);
            }
            PlaceGun(1, hand1p, nullptr);
            ShowBody(1, root1p, g_control.body != 0);
        }
    }
    if (g_hookFrames++ < 3) Log("hook t%u: applied pose %llu (mode %d)", GetCurrentThreadId(), g_appliedSeq, mode);
}

// SEH guard: a fault inside the hook's Snake code turns Snake mode off
// instead of crashing the game. (No C++ objects in this frame.)
bool SafeApply(void* self, float delta) {
    __try {
        ApplySnake(self, delta);
        return true;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return false;
    }
}

// Camera nodes: Skyrim's third-person camera follows "Camera3rd [Cam3]"
// (a root child, 121 units up) and the first-person one "Camera1st
// [Cam1]" (under the first-person NPC Root, 120.5 up). In Snake mode both
// are lowered with Snake's head (crouched, prone); their own heights come
// back when it ends.

void PlaceCameras(void* player, float drop) {
    const char* names[2] = {"Camera3rd [Cam3]", "Camera1st [Cam1]"};
    for (int v = 0; v < 2; ++v) {
        CameraNode& c = g_cameraNodes[v];
        void* root = Get3D(player, v == 1);
        if (root != c.root) {
            c = CameraNode{};
            c.root = root;
            c.node = root ? FindNode(root, names[v]) : nullptr;
            if (c.node) c.baseZ = At<float>(c.node, kLocalPos + 8);
        }
        if (c.node) At<float>(c.node, kLocalPos + 8) = c.baseZ + drop;
    }
}

// Facing: Snake decides where he faces (MGSV: he turns toward the
// camera-relative stick with his own turn rate and turn clips; aiming, toward
// the camera). The actor's heading is left to Skyrim, because its
// third-person camera keeps its yaw relative to it (writing the heading spun
// the camera). Instead "NPC Root [Root]" is turned by Snake's facing minus
// the actor's heading, replacing the turn Skyrim's animation graph puts
// there (which snapped back on stopping).
void FollowTurn(void* player) {
    static void* root = nullptr;
    static void* npcRoot = nullptr;
    void* r = Get3D(player, false);
    if (r != root) {
        root = r;
        npcRoot = r ? FindNode(r, "NPC Root [Root]") : nullptr;
    }
    if (!npcRoot) return;
    void* parent = At<void*>(npcRoot, kParent);
    if (!parent) return;
    // Local yaw d (heading convention: local +Y goes to (sin d, cos d)).
    float d = g_ownHeading ? g_snakeHeading - At<float>(player, kRefAngle + 8) : 0.0f;
    float c = std::cos(d), sn = std::sin(d);
    float lr[9] = {c, sn, 0, -sn, c, 0, 0, 0, 1};
    memcpy(&At<float>(npcRoot, kLocalRot), lr, sizeof lr);
    // World = parent world x local.
    const float* pr = &At<float>(parent, kWorldRot);
    const float* pp = &At<float>(parent, kWorldPos);
    float ps = At<float>(parent, kWorldScale);
    const float* lp = &At<float>(npcRoot, kLocalPos);
    float* wr = &At<float>(npcRoot, kWorldRot);
    float* wp = &At<float>(npcRoot, kWorldPos);
    MatMul(pr, lr, wr);
    for (int a = 0; a < 3; ++a) wp[a] = pp[a] + ps * (pr[a * 3 + 0] * lp[0] + pr[a * 3 + 1] * lp[1] + pr[a * 3 + 2] * lp[2]);
}

void RestoreCameras() {
    for (CameraNode& c : g_cameraNodes)
        if (c.node) At<float>(c.node, kLocalPos + 8) = c.baseZ;
}

bool g_camerasLowered = false;  // hook thread

void SafeRestoreCameras() {
    __try {
        RestoreCameras();
    } __except (EXCEPTION_EXECUTE_HANDLER) {
    }
}

void HookedUpdateAnimation(void* self, float delta) {
    g_originalUpdateAnimation(self, delta);
    g_hookCalls.fetch_add(1, std::memory_order_relaxed);
    g_playerFrames.fetch_add(1, std::memory_order_relaxed);
    if (!g_enabled || !g_retarget || self != ThePlayer()) {
        g_havePos = false;
        if (self == ThePlayer() && g_camerasLowered) {
            g_camerasLowered = false;
            SafeRestoreCameras();
        }

        if (self == ThePlayer() && (g_weaponNodes[0].hidden || g_weaponNodes[1].hidden)) {
            std::unique_lock<std::mutex> lock(g_hookMutex, std::try_to_lock);
            if (lock.owns_lock()) {
                SafeSetWeaponsHidden(self, false);
                ShowBody(0, Get3D(self, false), false);
                ShowBody(1, Get3D(self, true), false);
                for (int view = 0; view < 2; ++view) {
                    for (GunSlot& g : g_guns)
                        if (void* gun = g.node[view]) SetHidden(gun, true);
                    for (GunSlot& g : g_mags)
                        if (void* mag = g.node[view]) SetHidden(mag, true);
                    if (void* flash = g_flashNode[view]) SetHidden(flash, true);
                }
            }
        }
        return;
    }
    g_camerasLowered = true;
    std::unique_lock<std::mutex> lock(g_hookMutex, std::try_to_lock);
    if (!lock.owns_lock()) return;
    static bool hideFailed = false;
    if (!hideFailed && !SafeSetWeaponsHidden(self, true)) {
        hideFailed = true;
        Log("could not hide Skyrim's weapon nodes");
    }
    if (!SafeApply(self, delta)) {
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
    Log("hooked PlayerCharacter::UpdateAnimation (vtable %p, original %p); press F8 in game for Snake mode, F10 for write mode", vt,
        g_originalUpdateAnimation);
    return true;
}

// While Snake mode is on, Skyrim's attack/block handler ignores input, so
// the mouse buttons only aim and fire Snake's gun (slot 1 = CanProcess).
using CanProcessFn = bool (*)(void* self, void* event);
CanProcessFn g_originalAttackCanProcess = nullptr;

// R is MGSV's reload key and Skyrim's ready-weapon key: Skyrim's
// ReadyWeaponHandler is switched off in Snake mode the same way. Its vtable
// comes from PlayerControls, which holds every input handler: the
// ready-weapon handler six slots before the attack/block handler (found by
// looking for the attack handler this hook is called on).
CanProcessFn g_originalReadyCanProcess = nullptr;
// Jumping: off while Snake is crouched or prone (MGSV has no jump there).
CanProcessFn g_originalJumpCanProcess = nullptr;
// Sneaking: Snake's stance decides (Ctrl is read by the Snake thread).
CanProcessFn g_originalSneakCanProcess = nullptr;
// Sprinting: Snake dashes while the sprint key is held, without stamina.
CanProcessFn g_originalSprintCanProcess = nullptr;

bool HookedSprintCanProcess(void* self, void* event) {
    if (g_enabled) return false;
    return g_originalSprintCanProcess(self, event);
}

bool HookedSneakCanProcess(void* self, void* event) {
    CaptureSneakEvent(self, event);
    if (g_enabled) return false;
    return g_originalSneakCanProcess(self, event);
}

bool HookedJumpCanProcess(void* self, void* event) {
    // Moving, Space is Snake's dive.
    if (g_enabled && (g_snakeStance != 0 || g_snakeMoving)) return false;
    return g_originalJumpCanProcess(self, event);
}

// Patch slot 1 (CanProcess) of a handler's vtable.
bool HookCanProcess(void* handler, void* hook, CanProcessFn& original) {
    void** vt = Vtable(handler);
    DWORD old;
    if (!VirtualProtect(&vt[1], sizeof(void*), PAGE_EXECUTE_READWRITE, &old)) return false;
    original = reinterpret_cast<CanProcessFn>(vt[1]);
    vt[1] = hook;
    VirtualProtect(&vt[1], sizeof(void*), old, &old);
    return true;
}
std::atomic<bool> g_readySearched{false};

bool HookedReadyCanProcess(void* self, void* event) {
    if (g_enabled) return false;
    return g_originalReadyCanProcess(self, event);
}

void HookReadyWeapon(void* attackHandler) {
    void* controls = g_fire.playerControls && Readable(g_fire.playerControls, 8) ? *g_fire.playerControls : nullptr;
    if (!controls || !Readable(controls, 0x400)) {
        Log("ready weapon: no PlayerControls; R also readies Skyrim's weapon");
        return;
    }
    int slot = -1;
    for (int k = 6; k < 0x80; ++k)
        if (At<void*>(controls, k * 8) == attackHandler) slot = k;
    void* ready = slot > 0 ? At<void*>(controls, (slot - 6) * 8) : nullptr;
    if (!ready || !Readable(ready, 16) || !Readable(Vtable(ready), 16)) {
        Log("ready weapon: handler not found (attack handler at slot %d); R also readies Skyrim's weapon", slot);
        return;
    }
    const char* readyName = RttiName(ready);
    if (readyName && !strstr(readyName, "ReadyWeapon")) {
        Log("ready weapon: slot %d holds %s, not the ready-weapon handler", slot - 6, readyName);
        return;
    }
    if (HookCanProcess(ready, reinterpret_cast<void*>(&HookedReadyCanProcess), g_originalReadyCanProcess))
        Log("hooked %s::CanProcess (PlayerControls slot %d): R reloads in Snake mode", readyName ? readyName : "ReadyWeaponHandler",
            slot - 6);
    // The jump handler two slots before the attack handler.
    void* jump = At<void*>(controls, (slot - 2) * 8);
    const char* jumpName = jump ? RttiName(jump) : nullptr;
    if (jumpName && strstr(jumpName, "Jump") &&
        HookCanProcess(jump, reinterpret_cast<void*>(&HookedJumpCanProcess), g_originalJumpCanProcess))
        Log("hooked %s::CanProcess: no jumping while Snake is crouched or prone", jumpName);
    else
        Log("jump: handler not found (slot %d holds %s)", slot - 2, jumpName ? jumpName : "?");
    // The sneak handler two slots after it.
    void* sneak = At<void*>(controls, (slot + 2) * 8);
    const char* sneakName = sneak ? RttiName(sneak) : nullptr;
    if (sneakName && strstr(sneakName, "Sneak") &&
        HookCanProcess(sneak, reinterpret_cast<void*>(&HookedSneakCanProcess), g_originalSneakCanProcess)) {
        g_sneakHandler = sneak;
        Log("hooked %s::CanProcess: Ctrl sets Snake's stance in Snake mode", sneakName);
    }
    else
        Log("sneak: handler not found (slot %d holds %s)", slot + 2, sneakName ? sneakName : "?");
    // The sprint handler, before the ready-weapon one (checked by name).
    bool sprintHooked = false;
    for (int k = slot - 12; k < slot && !sprintHooked; ++k) {
        void* h = k >= 0 ? At<void*>(controls, k * 8) : nullptr;
        const char* name = h && Readable(h, 16) ? RttiName(h) : nullptr;
        if (name && strstr(name, "Sprint") &&
            HookCanProcess(h, reinterpret_cast<void*>(&HookedSprintCanProcess), g_originalSprintCanProcess)) {
            Log("hooked %s::CanProcess (slot %d): Snake dashes on the held sprint key, no stamina", name, k);
            sprintHooked = true;
        }
    }
    if (!sprintHooked) Log("sprint: handler not found; Skyrim's sprint also runs in Snake mode");
}

bool HookedAttackCanProcess(void* self, void* event) {
    if (!g_readySearched.exchange(true)) HookReadyWeapon(self);
    BuildSneakEvent(event);
    if (g_enabled) return false;
    return g_originalAttackCanProcess(self, event);
}

bool InstallInputHook() {
    void** vt = g_fire.attackBlockVtable;
    if (!vt) return false;
    DWORD old;
    if (!VirtualProtect(&vt[1], sizeof(void*), PAGE_EXECUTE_READWRITE, &old)) return false;
    g_originalAttackCanProcess = reinterpret_cast<CanProcessFn>(vt[1]);
    vt[1] = reinterpret_cast<void*>(&HookedAttackCanProcess);
    VirtualProtect(&vt[1], sizeof(void*), old, &old);
    Log("hooked AttackBlockHandler::CanProcess (vtable %p): Skyrim attacks/blocks are off in Snake mode", vt);
    return true;
}

void OnMessage(SKSEMessagingInterface::Message* msg) {
    // SpeedMult: never save Snake's value into the player's save, and re-read
    // the player's own value after a load or new game.
    if (msg->type == SKSEMessagingInterface::kMessage_SaveGame) {
        // Never save the crawl bonus (re-applied by the next tick).
        ApplyCrawlBonus(false);
        void* player = ThePlayer();
        float base = g_baseSpeedMult;
        if (player && base > 0 && g_speedMult > 0 && std::fabs(g_speedMult - base) > 0.01f &&
            SafeSetSpeedMult(player, base))
            g_speedMult = base;  // re-applied by the next speed update
        return;
    }
    if (msg->type == SKSEMessagingInterface::kMessage_PreLoadGame ||
        msg->type == SKSEMessagingInterface::kMessage_NewGame) {
        g_baseSpeedMult = -1.0f;
        g_speedMult = -1.0f;
        g_crawlBonusApplied = false;  // a save never holds it
        return;
    }
    if (msg->type == SKSEMessagingInterface::kMessage_DataLoaded) {
        Log("Skyrim data loaded");
        // A control file left by a test run would force its settings (aim
        // off, a stance) on play: start without one.
        if (DeleteFileW((PluginDir() + L"\\snake_control.txt").c_str())) Log("removed a leftover snake_control.txt");
        if (LoadSnakeDll() && OpenRuntime() && InstallHook()) {
            bool lib = LoadAddressLibrary();
            if (lib && !InstallInputHook()) Log("could not hook the attack/block handler");
            g_fire.ready = lib && SetUpFiring();
            Log("firing %s", g_fire.ready ? "ready" : "unavailable");
            SetUpSounds();
            SetUpAimCamera();
            g_running = true;
            g_snakeThread = std::thread(SnakeThread);
            g_snakeThread.detach();
        }
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
    g_tasks = static_cast<SKSETaskInterface*>(skse->QueryInterface(kInterface_Task));
    if (!g_tasks) Log("no task interface (firing disabled)");
    g_scaleform = static_cast<SKSEScaleformInterface*>(skse->QueryInterface(kInterface_Scaleform));
    if (!g_scaleform || !g_scaleform->Register("SnakeSKSE", RegisterScaleform)) Log("no Scaleform interface (no ammo HUD)");
    return true;
}

}  // extern "C"

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) g_self = module;
    return TRUE;
}
