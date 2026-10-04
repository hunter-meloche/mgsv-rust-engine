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
#include <chrono>
#include <cmath>
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <atomic>
#include <mutex>
#include <thread>
#include <string>
#include <unordered_map>
#include <vector>

#include "skse64/PluginAPI.h"
#include "skse64/gamethreads.h"
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

bool Readable(const void* p, size_t n) {
    MEMORY_BASIC_INFORMATION mbi{};
    if (!p || !VirtualQuery(p, &mbi, sizeof mbi)) return false;
    if (mbi.State != MEM_COMMIT || (mbi.Protect & (PAGE_NOACCESS | PAGE_GUARD))) return false;
    return reinterpret_cast<const char*>(p) + n <= reinterpret_cast<const char*>(mbi.BaseAddress) + mbi.RegionSize;
}

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
    g_layoutOk = checked > 0 && errA < 0.02 && errPos < 0.5;
    Log("layout check over %d bones: rot err (P*L) %.4f [worst %s], rot err (transposed) %.4f, pos err %.3f -> %s", checked,
        errA, worst, errB, errPos, g_layoutOk ? "OK, writes allowed" : "MISMATCH, writes disabled");
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

using LaunchFn = uint32_t* (*)(uint32_t* handle, LaunchData* data);
using LookupFn = void* (*)(uint32_t formId);

struct Firing {
    LaunchFn launch = nullptr;
    void* launchDataVtable = nullptr;
    void** camera = nullptr;  // &PlayerCamera singleton
    void** attackBlockVtable = nullptr;
    void* modelDemand = nullptr;
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
              demand = read(kIdModelDemand);
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
        bullet.speed = shot.speed * 70.0f;     // m/s -> units/s
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
// Played through XAudio2 with a small pool of voices per sound so full-auto
// shots overlap instead of cutting each other off. Volume: snake.ini
// ShotVolume (percent, default 70).

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
            for (int v = 0; v < 4; ++v) {
                IXAudio2SourceVoice* voice = nullptr;
                if (SUCCEEDED(g_audio->CreateSourceVoice(&voice, &s.format))) s.voices.push_back(voice);
            }
            Log("sound: %s shot %d: %u Hz x%u, %.2f s", id.c_str(), k, s.format.nSamplesPerSec, s.format.nChannels,
                s.data.size() / double(s.format.nAvgBytesPerSec));
            g_shotSounds[i].push_back(std::move(s));
        }
        if (g_shotSounds[i].empty()) Log("sound: no shot sound for %s (%s.wav)", id.c_str(), Narrow(base).c_str());
    }
    Log("sound: volume %.2f", volume);
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

// ---- Snake thread ---------------------------------------------------------------
//
// Snake's controller and the retarget run on this plugin's own thread at a
// fixed 60 Hz. Skyrim calls UpdateAnimation from varying job threads; running
// Rust code there hung the game, so the hook below never calls snake.dll: it
// only reports the player's position and copies the latest finished pose.

std::atomic<bool> g_enabled{false};
std::atomic<int> g_mode{2};  // 0 compute only, 1 local transforms, 2 local + world (default)
std::atomic<float> g_speed{0.0f};  // player speed (m/s), measured by the hook
// Actor::ActorState::actorState1: the ActorState base is at actor + 0xC0 on
// this runtime (vtable there, verified in game), flags follow at + 0xC8.
// Bits 0-3 moving back / forward / right / left, 6 walking, 7 running,
// 8 sprinting, 9 sneaking.
constexpr size_t kActorState1 = 0xC8;
std::atomic<uint32_t> g_actorFlags{0};
std::atomic<float> g_pitch{0.0f};  // camera (or actor) look pitch (radians, positive looks down)
std::atomic<bool> g_running{false};
std::atomic<uint32_t> g_hookCalls{0};  // player UpdateAnimation calls (for the status log)
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
    bool dump = false;   // log the camera once (diagnostics)
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
        else if (k == "dump") g_control.dump = v == "1";
    }
    fclose(f);
    Log("control: enabled %d mode %d move %s aim %d stance %s (current %d)", (int)g_enabled, (int)g_mode,
        g_control.move.c_str(), g_control.aim, g_control.stance.c_str(), curStance);
}

void SnakeThread() {
    // Crouch follows Skyrim's sneak; X toggles prone (C is Skyrim's
    // auto-move, Z its shout key).
    Key f8{VK_F8}, f9{VK_F10}, x{'X'};
    // 1 / 2 switch Snake's weapon (MRS-4 / WU S.Pistol), as in MGSV's
    // primary/secondary slots.
    Key weaponKeys[2] = {{'1'}, {'2'}};
    bool proneToggled = false;
    double sinceAim = 10.0;  // seconds since Snake last aimed
    bool handsDrawn = false;
    uint32_t lastFlags = 0xFFFFFFFF;
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
        if (f9.Pressed(focused)) {
            int m = (g_mode + 1) % 3;
            g_mode = m;
            Log("write mode %d (%s)", m, m == 0 ? "compute only" : m == 1 ? "local transforms" : "local + world transforms");
        }
        if (f8.Pressed(focused)) {
            g_enabled = !g_enabled;
            Log("Snake mode %s", g_enabled ? "ON" : "OFF");
        }
        if (!g_enabled) continue;

        float speed = g_speed;
        uint32_t flags = g_actorFlags;
        if (flags != lastFlags) {
            Log("actor state: moving %x walk %u run %u sprint %u sneak %u (speed %.2f m/s)", flags & 0xF, (flags >> 6) & 1,
                (flags >> 7) & 1, (flags >> 8) & 1, (flags >> 9) & 1, speed);
            lastFlags = flags;
        }
        bool moving = (flags & 0xF) != 0 || speed > 0.3f;
        bool sprint = (flags >> 8) & 1, walkFlag = (flags >> 6) & 1, runFlag = (flags >> 7) & 1;
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
        if (g_control.weapon >= 0) {
            in.switch_to = g_control.weapon;
            g_control.weapon = -1;
        }
        in.move_y = moving ? 1.0f : 0.0f;
        // Gait from Skyrim's own flags, falling back to the measured speed.
        if (sprint || (!walkFlag && !runFlag && speed > 5.5f)) in.sprint = true;
        else if (walkFlag || (!runFlag && speed < 2.6f)) in.walk = true;
        // Stance: X toggles prone; otherwise crouch while Skyrim sneaks.
        if (x.Pressed(focused)) {
            proneToggled = !proneToggled;
            Log("prone %s", proneToggled ? "on" : "off");
        }
        int want = proneToggled ? 2 : sneaking ? 1 : 0;
        if (g_control.stance == "stand") want = 0;
        else if (g_control.stance == "crouch") want = 1;
        else if (g_control.stance == "prone") want = 2;
        if (want != cur.stance) {
            if (want == 2) in.prone = true;                         // stand/crouch -> prone
            else if (cur.stance == 2) (want == 1 ? in.crouch : in.prone) = true;  // prone -> crouch / stand
            else in.crouch = true;                                  // stand <-> crouch
        }
        in.aim = g_control.aim >= 0 ? g_control.aim == 1 : focused && (GetAsyncKeyState(VK_RBUTTON) & 0x8000) != 0;
        in.fire = in.aim && (g_control.fire >= 0 ? g_control.fire == 1 : focused && (GetAsyncKeyState(VK_LBUTTON) & 0x8000) != 0);
        in.aim_pitch = -g_pitch.load();
        // Snake's own heading is "forward": Skyrim decides where the actor faces.
        SnakeVec3 snakePos{};
        float snakeYaw = 0;
        g_snake.player_transform(g_player, &snakePos, &snakeYaw);
        in.camera_yaw = snakeYaw;
        if (!g_snake.player_update(g_player, &in, nullptr, static_cast<float>(dt))) {
            Log("snake_player_update failed: %s", g_snake.last_error());
            g_enabled = false;
            continue;
        }
        {
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
        for (uint32_t i = 0; i < weapons; ++i)
            gunPlace[i] = g_snake.retarget_weapon_place(g_retarget, g_player, i, &gunPos[i], &gunRot[i]);
        {
            std::lock_guard<std::mutex> lock(g_pose.m);
            g_pose.rot = rot;
            g_pose.tr = tr;
            g_pose.has = has;
            memcpy(g_pose.gunPlace, gunPlace, sizeof gunPlace);
            memcpy(g_pose.gunPos, gunPos, sizeof gunPos);
            memcpy(g_pose.gunRot, gunRot, sizeof gunRot);
            ++g_pose.seq;
        }
        logTimer += dt;
        if (logTimer > 2.0) {
            logTimer = 0;
            SnakeStatus st{};
            g_snake.player_status(g_player, &st);
            Log("snake: speed %.2f m/s stance %d gait %d aiming %d ammo %d/%d%s, shots %d, mode %d, focused %d, hook calls %u, hands %d, heading %.3f",
                speed, st.stance, st.gait, st.aiming, st.ammo, st.reserve, st.reloading ? " (reloading)" : "",
                (int)g_shotsFired, (int)g_mode, (int)focused, g_hookCalls.exchange(0), ThePlayer() ? WeaponStateOf(ThePlayer()) : -1,
                ThePlayer() ? At<float>(ThePlayer(), kRefAngle + 8) : 0.0f);
        }
    }
}

// ---- per-frame hook (Skyrim job threads) -----------------------------------------

std::mutex g_hookMutex;  // the player's hook state (try-locked, never waited on)
float g_lastPos[3] = {0, 0, 0};
bool g_havePos = false;
void* g_cachedRoot = nullptr;
std::vector<SnakeQuat> g_hookRot;
std::vector<SnakeVec3> g_hookTr;
std::vector<uint8_t> g_hookHas;
uint64_t g_appliedSeq = 0;
int g_hookFrames = 0;
// Per weapon mesh: 0 hidden, 1 in the hands, 2 holstered (spine-relative).
int g_hookGunPlace[4] = {};
SnakeVec3 g_hookGunPos[4] = {};
SnakeQuat g_hookGunRot[4] = {{0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}, {0, 0, 0, 1}};

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

// One mesh per weapon (weapon index order).
constexpr int kMaxGuns = 4;
struct GunSlot {
    std::atomic<void*> node{nullptr};    // loaded model root
    std::atomic<void*> parent{nullptr};  // hand node it is attached to
    bool failed = false;                 // no mesh (run `mgsv skyrim-export`)
};
GunSlot g_guns[kMaxGuns];
std::atomic<int> g_gunCount{0};  // set once every mesh has been tried
// The muzzle flash is a separate mesh beside the gun (inside the gun's node
// its additive shapes made Skyrim draw the whole gun unlit).
constexpr const char* kFlashMesh = "snake\\muzzle_flash.nif";
std::atomic<void*> g_flashNode{nullptr};
std::atomic<void*> g_flashParent{nullptr};
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

void SetHidden(void* node, bool hidden);

class AttachGunTask : public TaskDelegate {
public:
    void Run() override {
        g_gunQueued = false;
        void* player = ThePlayer();
        void* root = player ? Get3D(player, false) : nullptr;
        void* hand = root ? FindNode(root, kRightHand) : nullptr;
        if (!hand) return;
        int count = (std::min)((int)g_snake.weapon_count(g_runtime), kMaxGuns);
        for (int i = 0; i < count; ++i) {
            GunSlot& g = g_guns[i];
            if (!g.node && !g.failed) {
                const char* mesh = g_snake.weapon_mesh(g_runtime, i);
                void* node = nullptr;
                uint32_t err = mesh && g_fire.modelDemand ? SafeDemand(mesh, &node) : 0xFFFFFFFE;
                Log("gun model %s: error %u, node %p (%s)", mesh ? mesh : "?", err, node, node ? Name(node) : "-");
                if (err != 0 || !node) {
                    g.failed = true;
                    continue;
                }
                SetHidden(node, true);
                g.node = node;
            }
            void* gun = g.node;
            if (gun && g.parent != hand) {
                if (SafeAttach(hand, gun)) {
                    g.parent = hand;
                    Log("gun model %d attached to %s (%p)", i, kRightHand, hand);
                } else {
                    g.failed = true;
                    Log("attaching gun model %d faulted", i);
                }
            }
        }
        if (!g_flashNode && g_fire.modelDemand) {
            void* node = nullptr;
            uint32_t err = SafeDemand(kFlashMesh, &node);
            Log("muzzle flash model %s: error %u, node %p", kFlashMesh, err, node);
            if (err == 0 && node) g_flashNode = node;
            g_muzzles.clear();
            for (int i = 0; i < count; ++i) {
                SnakeVec3 m{};
                g_snake.weapon_muzzle(g_runtime, i, &m);
                g_muzzles.push_back(m);
            }
        }
        void* flash = g_flashNode;
        if (flash && g_flashParent != hand && SafeAttach(hand, flash)) {
            g_flashParent = hand;
            SetHidden(flash, true);
        }
        g_gunCount = count;
    }
    void Dispose() override { delete this; }
};

void SetHidden(void* node, bool hidden) {
    uint32_t& flags = At<uint32_t>(node, 0xF4);
    flags = hidden ? (flags | 1u) : (flags & ~1u);
}

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

// Place (or hide) every gun under `hand`; queue the attach when needed.
// Local transforms only: Skyrim derives the world transforms (and the
// renderer's per-object state) in its next update. Writing a gun's world
// transform here, as for the bones, made the renderer draw it as a flat
// fog-coloured silhouette on some frames.
void PlaceGun(void* hand, void* spine) {
    if (!hand) return;
    int count = g_gunCount;
    bool attached = count > 0;
    for (int i = 0; i < count; ++i)
        if (!g_guns[i].failed && g_guns[i].parent != hand) attached = false;
    if (!attached) {
        if (g_tasks && !g_gunQueued.exchange(true)) g_tasks->AddTask(new AttachGunTask());
        return;
    }
    int inHand = -1;
    for (int i = 0; i < count; ++i) {
        void* gun = g_guns[i].node;
        if (!gun) continue;
        int place = g_hookGunPlace[i];
        if (place == 0 || (place == 2 && !spine)) {
            SetHidden(gun, true);
            continue;
        }
        SetHidden(gun, false);
        HandLocal(place, g_hookGunPos[i], g_hookGunRot[i], hand, spine, &At<float>(gun, kLocalRot),
                  &At<float>(gun, kLocalPos));
        if (place == 1) inHand = i;
    }
    // Flash: same rotation as the gun in the hands, at its muzzle; one
    // random cell.
    void* flash = g_flashParent == hand ? static_cast<void*>(g_flashNode) : nullptr;
    if (!flash) return;
    bool flashing = inHand >= 0 && inHand < (int)g_muzzles.size() && NowMs() < g_flashUntil;
    SetHidden(flash, !flashing);
    if (!flashing) return;
    void* gun = g_guns[inHand].node;
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
        CheckLayout();
        g_hookFrames = 0;
    }

    // Player speed for Snake's gait.
    float* pos = &At<float>(player, kRefPosition);
    if (g_havePos && delta > 0) {
        float dx = pos[0] - g_lastPos[0], dy = pos[1] - g_lastPos[1];
        g_speed = std::sqrt(dx * dx + dy * dy) / delta / 70.0f;
    }
    {
        g_actorFlags = At<uint32_t>(player, kActorState1);
        // Aim pitch follows the camera (positive looks down), else the actor.
        float cam[3], dir[3];
        g_pitch = CameraAim(cam, dir) ? -std::asin((std::max)(-1.0f, (std::min)(1.0f, dir[2])))
                                      : At<float>(player, kRefAngle);
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
            g_appliedSeq = g_pose.seq;
        }
    }
    size_t n = (std::min)(g_nodes.size(), g_hookRot.size());
    for (size_t i = 0; i < n; ++i) {
        void* node = g_nodes[i];
        if (!node) continue;
        QuatToMat(g_hookRot[i], &At<float>(node, kLocalRot));
        if (g_hookHas[i]) {
            float* lp = &At<float>(node, kLocalPos);
            lp[0] = g_hookTr[i].x;
            lp[1] = g_hookTr[i].y;
            lp[2] = g_hookTr[i].z;
        }
    }
    // World transforms of the retargeted bones, parent-first (table order).
    for (size_t i = 0; i < n && mode >= 2; ++i) {
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
    if (mode >= 2) {
        void* hand = nullptr;
        void* spine = nullptr;
        for (size_t i = 0; i < n; ++i) {
            if (g_boneNames[i] == kRightHand) hand = g_nodes[i];
            if (g_boneNames[i] == "NPC Spine2 [Spn2]") spine = g_nodes[i];
        }
        PlaceGun(hand, spine);
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

void HookedUpdateAnimation(void* self, float delta) {
    g_originalUpdateAnimation(self, delta);
    g_hookCalls.fetch_add(1, std::memory_order_relaxed);
    if (!g_enabled || !g_retarget || self != ThePlayer()) {
        g_havePos = false;
        if (self == ThePlayer() && (g_weaponNodes[0].hidden || g_weaponNodes[1].hidden)) {
            std::unique_lock<std::mutex> lock(g_hookMutex, std::try_to_lock);
            if (lock.owns_lock()) {
                SafeSetWeaponsHidden(self, false);
                for (GunSlot& g : g_guns)
                    if (void* gun = g.node) SetHidden(gun, true);
                if (void* flash = g_flashNode) SetHidden(flash, true);
            }
        }
        return;
    }
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

bool HookedAttackCanProcess(void* self, void* event) {
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
    if (msg->type == SKSEMessagingInterface::kMessage_DataLoaded) {
        Log("Skyrim data loaded");
        if (LoadSnakeDll() && OpenRuntime() && InstallHook()) {
            bool lib = LoadAddressLibrary();
            if (lib && !InstallInputHook()) Log("could not hook the attack/block handler");
            g_fire.ready = lib && SetUpFiring();
            Log("firing %s", g_fire.ready ? "ready" : "unavailable");
            SetUpSounds();
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
    return true;
}

}  // extern "C"

BOOL APIENTRY DllMain(HMODULE module, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) g_self = module;
    return TRUE;
}
