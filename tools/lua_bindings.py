"""Rebuild the engine's native Lua binding tables from the Ghidra export.

Steps:
  1. ``python tools/lua_bindings.py modules``  -> re/out/lua_modules.txt
     (native module names from re/api/lua_native_api.json)
  2. ``re/Run-GhidraExports.ps1``             -> re/out/lua_bindings.jsonl
  3. ``python tools/lua_bindings.py pair``     -> re/api/lua_bindings.json

``pair`` replays each registering function with a small register tracker
(MOV/LEA/XOR/ADD/SUB/OR/SHL with immediates, register copies, stack argument
slots) and inspects the arguments of every call:

* a member-name string plus a function pointer -> ``Module.Member`` binding;
* a member-name string plus an integer in R8   -> constant ``Module.Member = n``.

The module is whichever module-name string was last passed to a call in the
same function. Only names the scripts actually use are kept.
"""

import collections
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
API = REPO / "re" / "api" / "lua_native_api.json"
OUT = REPO / "re" / "out"

_FAMILIES = {}
for base, names in {
    "RAX": "RAX EAX AX AL AH", "RCX": "RCX ECX CX CL CH", "RDX": "RDX EDX DX DL DH",
    "RBX": "RBX EBX BX BL BH", "RSI": "RSI ESI SI SIL", "RDI": "RDI EDI DI DIL",
    "RBP": "RBP EBP BP BPL", "RSP": "RSP ESP SP SPL",
}.items():
    for n in names.split():
        _FAMILIES[n] = base
for i in range(8, 16):
    for suf in ("", "D", "W", "B"):
        _FAMILIES[f"R{i}{suf}"] = f"R{i}"

VOLATILE = ("RAX", "RCX", "RDX", "R8", "R9", "R10", "R11")
ARG_REGS = ("RCX", "RDX", "R8", "R9")
MEM_RE = re.compile(r"\[(\w+)(?: \+ (-?0x[0-9a-f]+))?\]$", re.I)
STACK_RE = re.compile(r"ptr \[RSP \+ (0x[0-9a-f]+)\]", re.I)


def reg(op: str):
    return _FAMILIES.get(op.upper())


def imm(op: str):
    return int(op, 16) if re.fullmatch(r"-?0x[0-9a-fA-F]+", op) else None


class Str(str):
    pass


class Fn(str):
    pass


class Table(list):
    """A pointer table of enum names (registered in a loop)."""


class TableStr(str):
    """A name loaded from a Table; remembers the table it came from."""

    table: "Table"


def replay(instructions):
    """Yield (index, args) for every call, with tracked argument values."""
    regs: dict = {}
    stack: dict = {}
    for idx, (mn, ops, sref, fref, is_call, table) in enumerate(instructions):
        if is_call:
            args = [regs.get(r) for r in ARG_REGS]
            args += [stack.get(off) for off in sorted(stack)]
            yield idx, args
            for r in VOLATILE:
                regs.pop(r, None)
            stack.clear()
            continue
        if not ops:
            continue
        dst = reg(ops[0])
        src = ops[1] if len(ops) > 1 else None
        value = None
        mem_base = MEM_RE.search(src or "")
        if table is not None:
            value = Table(table)
        elif sref is not None and mn == "MOV" and mem_base and isinstance(regs.get(reg(mem_base.group(1)) or ""), Table):
            value = TableStr(sref)
            value.table = regs[reg(mem_base.group(1))]
        elif sref is not None:
            value = Str(sref)
        elif fref is not None and mn in ("LEA", "MOV"):
            value = Fn(fref)
        elif mn == "MOV" and src is not None:
            if imm(src) is not None:
                value = imm(src)
            elif reg(src):
                value = regs.get(reg(src))
        elif mn == "LEA" and src is not None:
            m = MEM_RE.search(src)
            if m and reg(m.group(1)) and isinstance(regs.get(reg(m.group(1))), int):
                value = regs[reg(m.group(1))] + (int(m.group(2), 16) if m.group(2) else 0)
        elif mn == "XOR" and src == ops[0]:
            value = 0
        elif mn in ("ADD", "SUB", "OR", "SHL") and dst and src and imm(src) is not None:
            cur = regs.get(dst)
            if isinstance(cur, int):
                k = imm(src)
                value = {"ADD": cur + k, "SUB": cur - k, "OR": cur | k, "SHL": cur << k}[mn]

        if dst:
            if value is None:
                regs.pop(dst, None)
            else:
                regs[dst] = value
        else:
            m = STACK_RE.search(ops[0])
            if m and mn == "MOV":
                v = value if value is not None else (regs.get(reg(src)) if src and reg(src) else None)
                stack[int(m.group(1), 16)] = v


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
    native = {k for k, v in mods.items() if v["in_exe"] and not v["defined_by_script"]}
    functions: dict = collections.defaultdict(dict)
    constants: dict = collections.defaultdict(dict)
    registrars: dict = collections.defaultdict(set)

    for line in (OUT / "lua_bindings.jsonl").read_text().splitlines():
        rec = json.loads(line)
        current = None
        ins = rec["ins"]
        for idx, args in replay(ins):
            strs = [a for a in args if isinstance(a, (Str, TableStr))]
            fns = [a for a in args if isinstance(a, Fn)]
            for s in strs:
                if s in native:
                    current = s
            if current is None:
                continue
            wanted = mods[current]["members"]
            for s in strs:
                if s not in wanted or s == current:
                    continue
                registrars[current].add(rec["func"])
                if fns:
                    functions[current].setdefault(s, fns[0])
                elif isinstance(s, TableStr) and type(args[2]) is int:
                    # Loop: value = counter, bounded by the CMP after the call.
                    base, count = args[2], len(s.table)
                    for mn, ops, *_ in ins[idx + 1 : idx + 8]:
                        if mn == "CMP" and len(ops) == 2 and imm(ops[1]) is not None:
                            count = imm(ops[1]) - base
                            break
                    for k, name in enumerate(s.table[:count]):
                        if name in wanted:
                            constants[current].setdefault(name, base + k)
                elif type(args[2]) is int:
                    constants[current].setdefault(s, args[2])

    result = {
        "functions": {m: dict(sorted(v.items())) for m, v in sorted(functions.items())},
        "constants": {m: dict(sorted(v.items())) for m, v in sorted(constants.items())},
        "registrars": {m: sorted(v) for m, v in sorted(registrars.items())},
    }
    dest = REPO / "re" / "api" / "lua_bindings.json"
    dest.write_text(json.dumps(result, indent=1))
    nf = sum(len(v) for v in functions.values())
    nc = sum(len(v) for v in constants.values())
    print(f"paired {nf} functions and {nc} constants across {len(registrars)} modules -> {dest}")


if __name__ == "__main__":
    {"modules": cmd_modules, "pair": cmd_pair}[sys.argv[1] if len(sys.argv) > 1 else "pair"]()
