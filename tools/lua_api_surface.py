"""Derive the native Lua API surface the game scripts depend on.

Scans every Lua script extracted from the user's install (``mgsv extract .lua``
and ``mgsv extract .fpkd --unpack-fpk``) for ``Global.member`` / ``Global:method``
references whose ``Global`` is not defined by a script, and cross-checks names
against strings in mgsvtpp.exe. The result is the list of engine-side modules,
functions and constants the Rust runtime has to provide.

Writes re/api/lua_native_api.json (committed; contains only identifiers).

Usage: python tools/lua_api_surface.py [--extracted DIR] [--exe PATH]
"""

import argparse
import collections
import json
import re
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

LUA_STDLIB = {
    "string", "table", "math", "os", "io", "coroutine", "debug", "package", "bit", "_G",
}
# Engine globals that are lowercase.
LOWER_NATIVE_HINTS = {"vars", "svars", "gvars", "mvars", "fox"}

IDENT = r"[A-Za-z_][A-Za-z0-9_]*"
ACCESS_RE = re.compile(rf"(?<![\w.:])({IDENT})\s*([.:])\s*({IDENT})(\s*[\(\{{\"'])?")
STRING_RE = re.compile(r"\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*'|\[(=*)\[.*?\]\1\]", re.S)
COMMENT_RE = re.compile(r"--\[(=*)\[.*?\]\1\]|--[^\n]*", re.S)
LOCAL_RE = re.compile(rf"\blocal\s+(?:function\s+)?({IDENT}(?:\s*,\s*{IDENT})*)")
PARAM_RE = re.compile(rf"\bfunction\b[^(]*\(([^)]*)\)")
FOR_RE = re.compile(rf"\bfor\s+({IDENT}(?:\s*,\s*{IDENT})*)\s+(?:in|=)")
GLOBAL_ASSIGN_RE = re.compile(rf"(?m)^\s*({IDENT})\s*=(?!=)")


def strip_strings_and_comments(src: str) -> str:
    # Remove comments first (they can contain quotes), then string literals.
    out = COMMENT_RE.sub(" ", src)
    return STRING_RE.sub('""', out)


def exe_strings(exe: Path) -> set[str]:
    data = exe.read_bytes()
    return {m.group().decode() for m in re.finditer(rb"[A-Za-z_][A-Za-z0-9_]{1,80}", data)}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--extracted", type=Path, default=REPO / "extracted")
    ap.add_argument("--exe", type=Path, default=REPO.parent / "mgsvtpp.exe")
    ap.add_argument("--out", type=Path, default=REPO / "re" / "api" / "lua_native_api.json")
    args = ap.parse_args()

    files = sorted(args.extracted.rglob("*.lua"))
    if not files:
        raise SystemExit("no Lua files found; run `mgsv extract .lua` and `mgsv extract .fpkd --unpack-fpk` first")

    script_modules = {f.stem for f in files}
    script_globals: set[str] = set()
    refs: dict[str, dict[str, collections.Counter]] = collections.defaultdict(lambda: collections.defaultdict(collections.Counter))
    files_using: dict[str, set[str]] = collections.defaultdict(set)

    sources = {}
    for f in files:
        src = strip_strings_and_comments(f.read_text("utf-8", errors="replace"))
        sources[f] = src
        script_globals.update(GLOBAL_ASSIGN_RE.findall(src))

    for f, src in sources.items():
        locals_ = set()
        for m in LOCAL_RE.finditer(src):
            locals_.update(n.strip() for n in m.group(1).split(","))
        for m in PARAM_RE.finditer(src):
            locals_.update(n.strip() for n in m.group(1).split(",") if n.strip() and n.strip() != "...")
        for m in FOR_RE.finditer(src):
            locals_.update(n.strip() for n in m.group(1).split(","))

        rel = f.relative_to(args.extracted).as_posix()
        for m in ACCESS_RE.finditer(src):
            g, sep, member, tail = m.group(1), m.group(2), m.group(3), m.group(4)
            if g in locals_ or g in LUA_STDLIB or g == "self":
                continue
            if g in script_modules:
                continue
            if not (g[0].isupper() or g in LOWER_NATIVE_HINTS):
                continue
            kind = "method" if sep == ":" else ("call" if tail else "field")
            refs[g][member][kind] += 1
            files_using[g].add(rel)

    known = exe_strings(args.exe) if args.exe.is_file() else set()

    modules = {}
    for g in sorted(refs):
        members = {}
        for name, kinds in sorted(refs[g].items()):
            members[name] = {
                "kinds": dict(kinds),
                "in_exe": name in known,
            }
        modules[g] = {
            "in_exe": g in known,
            "defined_by_script": g in script_globals,
            "files": len(files_using[g]),
            "members": members,
        }

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({"scripts_scanned": len(files), "modules": modules}, indent=1))

    native = {k: v for k, v in modules.items() if v["in_exe"] and not v["defined_by_script"]}
    n_members = sum(len(v["members"]) for v in native.values())
    n_funcs = sum(1 for v in native.values() for m in v["members"].values() if set(m["kinds"]) & {"call", "method"})
    print(f"scanned {len(files)} scripts")
    print(f"native modules: {len(native)} (of {len(modules)} candidate globals), members: {n_members}, callable: {n_funcs}")
    top = sorted(native.items(), key=lambda kv: -sum(sum(m["kinds"].values()) for m in kv[1]["members"].values()))
    for k, v in top[:40]:
        uses = sum(sum(m["kinds"].values()) for m in v["members"].values())
        print(f"  {k:<28} {len(v['members']):>4} members  {uses:>6} refs  {v['files']:>4} files")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
