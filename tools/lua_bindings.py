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
                if mn == "ADD":
                    value = cur + k
                elif mn == "SUB":
                    value = cur - k
                elif mn == "OR":
                    value = cur | k
                elif 0 <= k < 64:
                    value = cur << k

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
    return json.loads(API.read_text(encoding="utf-8"))["modules"]


CONST_NAME = re.compile(r"^[A-Z][A-Z0-9]*_[A-Za-z0-9_]+$")
STACK_SLOT = re.compile(r"(dword|qword) ptr \[(RSP|RBP) \+ (-?0x[0-9a-f]+)\]$", re.I)


def cmd_modules() -> None:
    mods = load_api()
    OUT.mkdir(parents=True, exist_ok=True)
    names = sorted(k for k, v in mods.items() if v["in_exe"] and not v["defined_by_script"])
    (OUT / "lua_modules.txt").write_text("\n".join(names) + "\n", encoding="utf-8")
    # Constant-style member names (EQP_WP_..., PHASE_ALERT) find functions
    # that register enums without naming their module.
    consts = sorted({m for k in names for m in mods[k]["members"] if CONST_NAME.match(m)})
    (OUT / "lua_members.txt").write_text("\n".join(consts) + "\n", encoding="utf-8")
    print(f"{len(names)} module names, {len(consts)} constant names -> {OUT}")


def stack_enum_pairs(instructions):
    """Yield (name, value) from arrays of {u32 value; char* name} built on the stack."""
    strings: dict = {}
    values: dict = {}
    for mn, ops, sref, _fref, is_call, _table in instructions:
        if is_call:
            strings.clear()
            continue
        dst = reg(ops[0]) if ops else None
        if mn == "LEA" and dst and sref is not None:
            strings[dst] = sref
            continue
        m = STACK_SLOT.match(ops[0]) if mn == "MOV" and len(ops) == 2 else None
        if not m:
            if dst:
                strings.pop(dst, None)
            continue
        slot = (m.group(2).upper(), int(m.group(3), 16))
        if m.group(1).lower() == "dword" and imm(ops[1]) is not None:
            values[slot] = imm(ops[1])
        elif m.group(1).lower() == "qword" and reg(ops[1]) in strings:
            prev = (slot[0], slot[1] - 8)
            if prev in values:
                yield strings[reg(ops[1])], values[prev]


def cmd_pair() -> None:
    mods = load_api()
    native = {k for k, v in mods.items() if v["in_exe"] and not v["defined_by_script"]}
    functions: dict = collections.defaultdict(dict)
    constants: dict = collections.defaultdict(dict)
    registrars: dict = collections.defaultdict(set)

    owners: dict = collections.defaultdict(set)
    for k in native:
        for m in mods[k]["members"]:
            owners[m].add(k)

    records = [json.loads(l) for l in (OUT / "lua_bindings.jsonl").read_text(encoding="utf-8").splitlines()]
    members_file = OUT / "lua_members.jsonl"
    if members_file.exists():
        records += [json.loads(l) for l in members_file.read_text(encoding="utf-8").splitlines()]

    # Enum arrays built on the stack; attribute names to their unique owner.
    for rec in records:
        for name, value in stack_enum_pairs(rec["ins"]):
            mods_for = {m for m in owners.get(name, set()) if "index" not in mods[m]["members"][name]["kinds"]}
            if len(mods_for) == 1:
                (mod,) = mods_for
                constants[mod].setdefault(name, value)
                registrars[mod].add(rec["func"])

    for rec in records:
        if rec.get("parent") is None and not (set(rec["modules"]) & native):
            continue  # found via member names only; handled above
        # Callbacks of class-style modules never name the module; they inherit
        # it from the registrar that handed them out.
        current = None
        if rec.get("parent") and len(rec["modules"]) == 1 and rec["modules"][0] in native:
            current = rec["modules"][0]
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
                elif "index" in wanted[s]["kinds"]:
                    continue  # a namespace table (TppCommand.Weather), not a constant
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
    dest.write_text(json.dumps(result, indent=1), encoding="utf-8")
    nf = sum(len(v) for v in functions.values())
    nc = sum(len(v) for v in constants.values())
    print(f"paired {nf} functions and {nc} constants across {len(registrars)} modules -> {dest}")


if __name__ == "__main__":
    {"modules": cmd_modules, "pair": cmd_pair}[sys.argv[1] if len(sys.argv) > 1 else "pair"]()
