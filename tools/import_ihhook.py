"""Import engine function names from IHHook (MIT, TinManTex and contributors).

IHHook (https://github.com/kapuragu/IHHook) ships Ghidra-derived address
tables per game version, with the decompiled signature as a comment:

    {"FoxBlockLoad", 0x14006e040},//int * fox::Block::Load(void *thisPtr, ...)

This keeps the entries whose address is a function start in our own analysis
of the user's exe (so a version mismatch cannot mislabel anything) and writes
re/api/ihhook_names.json and re/out/ihhook_names.csv (for ApplyNames.java).

Usage: python tools/import_ihhook.py <path to IHHook checkout> [version]
       version defaults to 1_0_15_4_en
"""

import csv
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ENTRY = re.compile(r'\{\s*"(\w+)"\s*,\s*(0x[0-9a-fA-F]+)\s*\}\s*,?\s*(?://\s*(.*))?')


def main() -> None:
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    ihhook = Path(sys.argv[1])
    version = sys.argv[2] if len(sys.argv) > 2 else "1_0_15_4_en"
    header = ihhook / "IHHook" / "hooks" / f"mgsvtpp_adresses_{version}.h"

    functions = {
        int(row["entry"], 16)
        for row in csv.DictReader((REPO / "re/out/functions.csv").open(encoding="utf-8"))
    }

    names, skipped = [], 0
    for line in header.read_text(encoding="utf-8", errors="replace").splitlines():
        m = ENTRY.search(line)
        if not m:
            continue
        name, addr, sig = m.group(1), int(m.group(2), 16), (m.group(3) or "").strip()
        if addr not in functions:
            skipped += 1
            continue
        names.append({"name": name, "address": f"{addr:x}", "signature": sig if sig != name else ""})

    out = {
        "source": "IHHook (MIT, TinManTex and contributors), " + header.name,
        "game_version": version,
        "functions": sorted(names, key=lambda n: n["address"]),
    }
    dest = REPO / "re/api/ihhook_names.json"
    dest.write_text(json.dumps(out, indent=1), encoding="utf-8")
    with (REPO / "re/out/ihhook_names.csv").open("w", encoding="utf-8", newline="") as f:
        w = csv.writer(f)
        for n in names:
            w.writerow([n["address"], n["name"], n["signature"]])
    print(f"{len(names)} names verified against this exe ({skipped} skipped) -> {dest}")


if __name__ == "__main__":
    main()
