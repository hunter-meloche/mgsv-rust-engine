#!/usr/bin/env bash
# Run the export scripts against the analysed project (see run-ghidra-analysis.sh).
# Usage: re/run-ghidra-exports.sh [Script.java ...]   (default: all exports)
set -euo pipefail
cd "$(dirname "$0")/.."
GHIDRA="${GHIDRA_HOME:-$(ls -d .tools/ghidra_*_PUBLIC | tail -1)}"
export GHIDRA_HEADLESS_MAXMEM="${GHIDRA_HEADLESS_MAXMEM:-24G}"
ROOT="$(pwd -W)"

python tools/lua_bindings.py modules

scripts=("$@")
if [ ${#scripts[@]} -eq 0 ]; then
  scripts=(ExportFunctions.java ExportLuaBindings.java)
fi
post=()
for s in "${scripts[@]}"; do
  case "$s" in
    ExportFunctions.java) post+=(-postScript "$s" "$ROOT/re/out/functions.csv") ;;
    ExportLuaBindings.java) post+=(-postScript "$s" "$ROOT/re/out/lua_modules.txt" "$ROOT/re/out/lua_bindings.jsonl") ;;
    DecompileFunctions.java) post+=(-postScript "$s" "$ROOT/re/out/decompile_targets.txt" "$ROOT/re/out/decomp") ;;
    *) post+=(-postScript "$s") ;;
  esac
done

"$GHIDRA/support/analyzeHeadless.bat" "$ROOT/re/projects" mgsvtpp \
  -process mgsvtpp.exe -noanalysis -readOnly \
  -scriptPath "$ROOT/re/ghidra" "${post[@]}"

if [ -f re/out/lua_bindings.jsonl ]; then
  python tools/lua_bindings.py pair
fi
