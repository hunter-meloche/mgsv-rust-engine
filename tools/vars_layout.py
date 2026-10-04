"""Recover the engine-declared ``vars`` layout from the Ghidra export.

The engine declares every ``vars.*`` variable in one function by appending a
24-byte descriptor to a global table:

    +0x00  pointer to storage (vars block base + offset)
    +0x08  name (char*)
    +0x10  StrCode32(name)
    +0x14  flags << 24 | type << 16 | array_size     (type = TppScriptVars.TYPE_*)

Input : re/out/vars_decl.jsonl (ExportLuaBindings.java run over the ``vars``
        member names; see docs/ARCHITECTURE.md)
Output: re/api/vars_layout.json (identifiers and layout numbers only)
"""

import json
import re
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
TYPE_NAMES = {0: "BOOL", 1: "INT32", 2: "UINT32", 3: "FLOAT", 4: "INT8", 5: "UINT8", 6: "INT16", 7: "UINT16"}
HEX = re.compile(r"-?0x[0-9a-fA-F]+$")


def storage_offset(ins, start, name_idx):
    """Offset of the storage pointer written to descriptor +0x00.

    Finds ``MOV qword ptr [RAX + RCX*0x8], <reg>`` before the name, then the
    load of the vars base into <reg> and every ADD/SUB applied after it (the
    compiler writes +0x80 as ``SUB reg, -0x80``).
    """
    store = None
    for j in range(name_idx - 1, start - 1, -1):
        mn, ops, *_ = ins[j]
        if mn == "MOV" and len(ops) == 2 and ops[0].endswith("*0x8]") and "+ 0x" not in ops[0]:
            store = j
            break
    if store is None:
        raise ValueError(f"no storage pointer before instruction {name_idx}")
    reg = ins[store][1][1]
    offset = 0
    for j in range(store - 1, start - 1, -1):
        mn, ops, *_ = ins[j]
        if not ops or ops[0] != reg:
            continue
        if mn in ("ADD", "SUB") and len(ops) == 2 and HEX.match(ops[1]):
            k = int(ops[1], 16)
            offset += k if mn == "ADD" else -k
        elif mn == "MOV":
            return offset  # base load reached
        else:
            raise ValueError(f"unhandled {mn} {ops} computing storage offset")
    raise ValueError("base load not found")


def main() -> None:
    recs = [json.loads(l) for l in (REPO / "re/out/vars_decl.jsonl").read_text(encoding="utf-8").splitlines()]
    decl = max(recs, key=lambda r: len(r["modules"]))  # the function that names them all
    ins = decl["ins"]

    out = []
    last_desc = 0
    for k, (mn, ops, sref, *_rest) in enumerate(ins):
        if not (mn == "LEA" and sref and ops and ops[0] == "RDX"):
            continue
        hsh = packed = None
        for m2, o2, *_ in ins[k + 1 : k + 5]:
            if m2 == "MOV" and len(o2) == 2 and HEX.match(o2[1]):
                if o2[0].endswith("+ 0x10]"):
                    hsh = int(o2[1], 16)
                elif o2[0].endswith("+ 0x14]"):
                    packed = int(o2[1], 16)
        if packed is None:
            continue
        offset = storage_offset(ins, last_desc, k)
        last_desc = k
        ty = (packed >> 16) & 0xFF
        out.append(
            {
                "name": sref,
                "offset": offset,
                "type": TYPE_NAMES.get(ty, f"UNKNOWN_{ty}"),
                "array_size": packed & 0xFFFF,
                "flags": packed >> 24,
                "str_code32": hsh,
            }
        )

    dest = REPO / "re/api/vars_layout.json"
    dest.write_text(json.dumps({"declared_by": decl["func"], "vars": out}, indent=1), encoding="utf-8")
    used = set(json.loads((REPO / "re/api/lua_native_api.json").read_text(encoding="utf-8"))["modules"]["vars"]["members"])
    names = {v["name"] for v in out}
    print(f"{len(out)} vars -> {dest}")
    print(f"script-referenced vars covered: {len(used & names)}/{len(used)}; missing: {sorted(used - names)}")


if __name__ == "__main__":
    main()
