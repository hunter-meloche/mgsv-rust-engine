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
#include <share.h>

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
    decltype(&snake_player_new) player_new = nullptr;
    decltype(&snake_player_update) player_update = nullptr;
    decltype(&snake_player_status) player_status = nullptr;
    decltype(&snake_player_transform) player_transform = nullptr;
    decltype(&snake_player_take_shots) player_take_shots = nullptr;
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
    RESOLVE(player_take_shots, "snake_player_take_shots");
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
// reload, recoil). Each shot becomes a real Skyrim projectile launched on the
// main thread through SKSE's task interface, so Skyrim handles hits against
// the world, damage, kills and NPC reactions. For now the projectile, damage
// and speed come from a vanilla bow + arrow (Hunting Bow 0x13985, Iron Arrow
// 0x1397D); a dedicated bullet record comes later.
//
// Addresses: Projectile::Launch and the LaunchData vtable from the Address
// Library database (versionlib-1-7-104-0.bin: flat u32 offset table indexed
// by ID after a 0x60-byte header); LookupFormByID from SKSE 2.3.1 (0x1E5860).

constexpr uint32_t kIdProjectileLaunch = 44108;   // Projectile::Launch (AE id)
constexpr uint32_t kIdLaunchDataVtable = 187478;  // VTABLE_Projectile__LaunchData (AE id)
constexpr uintptr_t kLookupFormByID = 0x001E5860;
constexpr uint32_t kHuntingBow = 0x00013985, kIronArrow = 0x0001397D;
constexpr size_t kFormType = 0x1A;
constexpr size_t kAmmoProjectile = 0x110;  // TESAmmo AMMO_DATA.projectile
constexpr uint8_t kTypeWeapon = 0x29, kTypeAmmo = 0x2A, kTypeProjectile = 0x32, kTypeCell = 0x3C;

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

using LaunchFn = uint32_t* (*)(uint32_t* handle, LaunchData* data);
using LookupFn = void* (*)(uint32_t formId);

struct Firing {
    LaunchFn launch = nullptr;
    void* launchDataVtable = nullptr;
    void* bow = nullptr;
    void* arrow = nullptr;
    void* projectile = nullptr;
    size_t cellOffset = 0;
    bool ready = false;
} g_fire;

SKSETaskInterface* g_tasks = nullptr;
std::atomic<int> g_pendingShots{0};
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
    uintptr_t launch = read(kIdProjectileLaunch), vt = read(kIdLaunchDataVtable);
    fclose(f);
    if (!launch || !vt) {
        Log("firing: ids missing from the database");
        return false;
    }
    g_fire.launch = reinterpret_cast<LaunchFn>(base + launch);
    g_fire.launchDataVtable = reinterpret_cast<void*>(base + vt);
    Log("firing: %u ids; Projectile::Launch +%llX, LaunchData vtable +%llX", count, (unsigned long long)launch,
        (unsigned long long)vt);
    return true;
}

bool SetUpFiring() {
    if (!LoadAddressLibrary()) return false;
    auto lookup = reinterpret_cast<LookupFn>(reinterpret_cast<uintptr_t>(GetModuleHandleW(nullptr)) + kLookupFormByID);
    g_fire.bow = lookup(kHuntingBow);
    g_fire.arrow = lookup(kIronArrow);
    g_fire.projectile = g_fire.arrow && Readable(g_fire.arrow, kAmmoProjectile + 8) ? At<void*>(g_fire.arrow, kAmmoProjectile) : nullptr;
    Log("firing: bow %p (type %02X), arrow %p (type %02X), projectile %p (type %02X)", g_fire.bow, FormType(g_fire.bow),
        g_fire.arrow, FormType(g_fire.arrow), g_fire.projectile, FormType(g_fire.projectile));
    if (FormType(g_fire.bow) != kTypeWeapon || FormType(g_fire.arrow) != kTypeAmmo || FormType(g_fire.projectile) != kTypeProjectile) {
        Log("firing: form types do not match; firing disabled");
        return false;
    }
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

bool LaunchOne(void* player) {
    __try {
        void* cell = PlayerCell(player);
        if (!cell) return false;
        LaunchData d{};
        d.vtable = g_fire.launchDataVtable;
        // From the right hand, along the player's aim.
        const float* angle = &At<float>(player, kRefAngle);  // pitch, roll, heading
        float pitch = angle[0], heading = angle[2];
        float dir[3] = {std::sin(heading) * std::cos(pitch), std::cos(heading) * std::cos(pitch), -std::sin(pitch)};
        float* pos = &At<float>(player, kRefPosition);
        float origin[3] = {pos[0] + dir[0] * 60.0f, pos[1] + dir[1] * 60.0f, pos[2] + 110.0f};
        for (size_t i = 0; i < g_nodes.size() && i < g_boneNames.size(); ++i) {
            if (g_boneNames[i] == "NPC R Hand [RHnd]" && g_nodes[i]) {
                float* hand = &At<float>(g_nodes[i], kWorldPos);
                for (int k = 0; k < 3; ++k) origin[k] = hand[k] + dir[k] * 50.0f;
            }
        }
        memcpy(d.origin, origin, sizeof origin);
        d.projectileBase = g_fire.projectile;
        d.shooter = player;
        d.weaponSource = g_fire.bow;
        d.ammoSource = g_fire.arrow;
        d.angleZ = heading;
        d.angleX = pitch;
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
        void* player = ThePlayer();
        int n = g_pendingShots.exchange(0);
        for (int i = 0; i < n && player && g_fire.ready; ++i) {
            if (!LaunchOne(player)) {
                Log("firing: launch faulted; firing disabled");
                g_fire.ready = false;
                break;
            }
            int total = ++g_shotsFired;
            if (total <= 3 || total % 30 == 0) Log("firing: shot %d launched", total);
        }
    }
    void Dispose() override { delete this; }
};

void QueueShots(int n) {
    if (n <= 0 || !g_fire.ready || !g_tasks) return;
    if (g_pendingShots.fetch_add(n) == 0) g_tasks->AddTask(new ShotTask());
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
std::atomic<float> g_pitch{0.0f};  // actor look pitch (radians, positive looks down)
std::atomic<bool> g_running{false};
std::atomic<uint32_t> g_hookCalls{0};  // player UpdateAnimation calls (for the status log)
std::thread g_snakeThread;

struct PoseBuffer {
    std::mutex m;
    std::vector<SnakeQuat> rot;
    std::vector<SnakeVec3> tr;
    std::vector<uint8_t> has;
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
    }
    fclose(f);
    Log("control: enabled %d mode %d move %s aim %d stance %s (current %d)", (int)g_enabled, (int)g_mode,
        g_control.move.c_str(), g_control.aim, g_control.stance.c_str(), curStance);
}

void SnakeThread() {
    // Crouch follows Skyrim's sneak; X toggles prone (C is Skyrim's
    // auto-move, Z its shout key).
    Key f8{VK_F8}, f9{VK_F10}, x{'X'};
    bool proneToggled = false;
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
            if (k) QueueShots(static_cast<int>(k));
        }
        if (g_snake.retarget_apply(g_retarget, g_player, rot.data(), tr.data(), has.data(), n) != n) {
            Log("snake_retarget_apply failed: %s", g_snake.last_error());
            g_enabled = false;
            continue;
        }
        {
            std::lock_guard<std::mutex> lock(g_pose.m);
            g_pose.rot = rot;
            g_pose.tr = tr;
            g_pose.has = has;
            ++g_pose.seq;
        }
        logTimer += dt;
        if (logTimer > 2.0) {
            logTimer = 0;
            SnakeStatus st{};
            g_snake.player_status(g_player, &st);
            Log("snake: speed %.2f m/s stance %d gait %d aiming %d ammo %d/%d%s, shots %d, mode %d, focused %d, hook calls %u",
                speed, st.stance, st.gait, st.aiming, st.ammo, st.reserve, st.reloading ? " (reloading)" : "",
                (int)g_shotsFired, (int)g_mode, (int)focused, g_hookCalls.exchange(0));
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
        g_pitch = At<float>(player, kRefAngle);
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
        return;
    }
    std::unique_lock<std::mutex> lock(g_hookMutex, std::try_to_lock);
    if (!lock.owns_lock()) return;
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

void OnMessage(SKSEMessagingInterface::Message* msg) {
    if (msg->type == SKSEMessagingInterface::kMessage_DataLoaded) {
        Log("Skyrim data loaded");
        if (LoadSnakeDll() && OpenRuntime() && InstallHook()) {
            g_fire.ready = SetUpFiring();
            Log("firing %s", g_fire.ready ? "ready" : "unavailable");
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
