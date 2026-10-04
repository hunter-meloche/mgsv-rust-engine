"""Rebuild the engine's native Lua binding tables from the Ghidra export.

Steps:
  1. ``python tools/lua_bindings.py modules``  -> re/out/lua_modules.txt
     (native module names from re/api/lua_native_api.json)
  2. ``re/run-ghidra-exports.sh``             -> re/out/lua_bindings.jsonl
  3. ``python tools/lua_bindings.py pair``     -> re/api/lua_bindings.json

``pair`` walks each registering function's event stream. A member-name string
followed by a function reference becomes ``Module.Member -> address``; an
immediate followed by a member-name string becomes a constant. Only names
that the scripts actually use are kept, which filters out unrelated strings.
"""

import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
API = REPO / "re" / "api" / "lua_native_api.json"
OUT = REPO / "re" / "out"


def load_api() -> dict:
    return json.loads(API.read_text())["modules"]


def cmd_modules() -> None:
    mods = load_api()
    OUT.mkdir(parents=True, exist_ok=True)
    names = sorted(k for k, v in mods.items() if v["in_exe"] and not v["defined_by_script"])
    (OUT / "lua_modules.txt").write_text("\n".join(names) + "\n")
    print(f"{len(names)} module names -> {OUT / 'lua_modules.txt'}")


def cmd_pair() -> None:
    mods = load_api()
    functions: dict[str, dict[str, str]] = {}
    constants: dict[str, dict[str, int]] = {}
    src = OUT / "lua_bindings.jsonl"
    for line in src.read_text().splitlines():
        rec = json.loads(line)
        for module in rec["modules"]:
            wanted = set(mods.get(module, {}).get("members", {}))
            ev = rec["events"]
            for i, e in enumerate(ev):
                if e[0] != "s" or e[1] not in wanted:
                    continue
                name = e[1]
                # Function pointer loaded right before or after the name.
                for j in (i + 1, i - 1, i + 2, i - 2):
                    if 0 <= j < len(ev) and ev[j][0] == "f":
                        functions.setdefault(module, {}).setdefault(name, ev[j][1])
                        break
                else:
                    # Constant: nearest preceding immediate.
                    for j in range(i - 1, max(i - 4, -1), -1):
                        if ev[j][0] == "i":
                            constants.setdefault(module, {}).setdefault(name, ev[j][1])
                            break
    result = {"functions": functions, "constants": constants}
    dest = REPO / "re" / "api" / "lua_bindings.json"
    dest.write_text(json.dumps(result, indent=1, sort_keys=True))
    nf = sum(len(v) for v in functions.values())
    nc = sum(len(v) for v in constants.values())
    print(f"paired {nf} functions and {nc} constants across {len(set(functions) | set(constants))} modules -> {dest}")


if __name__ == "__main__":
    {"modules": cmd_modules, "pair": cmd_pair}[sys.argv[1] if len(sys.argv) > 1 else "pair"]()
