// SnakeSKSE: brings Snake's player systems (snake.dll, built from this repo's
// Rust crates) into Skyrim Special Edition through SKSE.
//
// Phase A: load snake.dll from this plugin's folder, open the MGSV runtime
// from the install named in snake.ini once Skyrim's data is loaded, and log
// what happened to Documents/My Games/Skyrim Special Edition/SKSE/SnakeSKSE.log.

#include <windows.h>
#include <shlobj.h>
#include <share.h>

#include <chrono>
#include <cstdarg>
#include <cstdio>
#include <string>

#include "skse64/PluginAPI.h"
#include "skse64_common/skse_version.h"

#include "snake.h"

namespace {

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

// snake.dll entry points, resolved at runtime so the DLL can live next to
// this plugin (Data/SKSE/Plugins) instead of the game's folder.
struct SnakeApi {
    decltype(&snake_last_error) last_error = nullptr;
    decltype(&snake_runtime_open) runtime_open = nullptr;
    decltype(&snake_bone_count) bone_count = nullptr;
    decltype(&snake_bone_name) bone_name = nullptr;
    decltype(&snake_weapon_count) weapon_count = nullptr;
    decltype(&snake_player_new) player_new = nullptr;
    decltype(&snake_player_update) player_update = nullptr;
    decltype(&snake_player_status) player_status = nullptr;
} g_snake;

SnakeRuntime* g_runtime = nullptr;
SnakePlayer* g_player = nullptr;

bool LoadSnakeDll() {
    std::wstring dll = PluginDir() + L"\\snake.dll";
    HMODULE h = LoadLibraryW(dll.c_str());
    if (!h) {
        Log("could not load %s (error %lu)", Narrow(dll).c_str(), GetLastError());
        return false;
    }
#define RESOLVE(field, name)                                                  \
    g_snake.field = reinterpret_cast<decltype(g_snake.field)>(GetProcAddress(h, name)); \
    if (!g_snake.field) { Log("snake.dll is missing %s", name); return false; }
    RESOLVE(last_error, "snake_last_error");
    RESOLVE(runtime_open, "snake_runtime_open");
    RESOLVE(bone_count, "snake_bone_count");
    RESOLVE(bone_name, "snake_bone_name");
    RESOLVE(weapon_count, "snake_weapon_count");
    RESOLVE(player_new, "snake_player_new");
    RESOLVE(player_update, "snake_player_update");
    RESOLVE(player_status, "snake_player_status");
#undef RESOLVE
    Log("loaded %s", Narrow(dll).c_str());
    return true;
}

void OpenRuntime() {
    std::wstring ini = PluginDir() + L"\\snake.ini";
    std::string game = IniString(ini, L"MgsvDir");
    std::string dicts = IniString(ini, L"DictionariesDir");
    if (game.empty() || dicts.empty()) {
        Log("set MgsvDir and DictionariesDir in %s", Narrow(ini).c_str());
        return;
    }
    Log("opening MGSV runtime: MgsvDir=%s DictionariesDir=%s", game.c_str(), dicts.c_str());
    auto t0 = std::chrono::steady_clock::now();
    g_runtime = g_snake.runtime_open(game.c_str(), dicts.c_str());
    double secs = std::chrono::duration<double>(std::chrono::steady_clock::now() - t0).count();
    if (!g_runtime) {
        Log("snake_runtime_open failed after %.1fs: %s", secs, g_snake.last_error());
        return;
    }
    uint32_t bones = g_snake.bone_count(g_runtime);
    Log("runtime open in %.1fs: %u bones (bone 0 = %s), %u weapons", secs, bones,
        g_snake.bone_name(g_runtime, 0) ? g_snake.bone_name(g_runtime, 0) : "?", g_snake.weapon_count(g_runtime));

    // Smoke test: one second of running forward, headless.
    g_player = g_snake.player_new(g_runtime);
    if (!g_player) {
        Log("snake_player_new failed: %s", g_snake.last_error());
        return;
    }
    SnakeInput input{};
    input.move_y = 1.0f;
    input.switch_to = -1;
    for (int i = 0; i < 60; ++i) g_snake.player_update(g_player, &input, nullptr, 1.0f / 60.0f);
    SnakeStatus st{};
    g_snake.player_status(g_player, &st);
    Log("player smoke test: stance %d gait %d weapon %d ammo %d/%d", st.stance, st.gait, st.weapon, st.ammo, st.reserve);
}

void OnMessage(SKSEMessagingInterface::Message* msg) {
    if (msg->type == SKSEMessagingInterface::kMessage_DataLoaded) {
        Log("Skyrim data loaded");
        if (LoadSnakeDll()) OpenRuntime();
    }
}

}  // namespace

extern "C" {

__declspec(dllexport) SKSEPluginVersionData SKSEPlugin_Version = {
    SKSEPluginVersionData::kVersion,
    1,
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
