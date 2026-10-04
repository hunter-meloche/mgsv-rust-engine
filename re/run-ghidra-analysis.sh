#!/usr/bin/env bash
# One-time Ghidra auto-analysis of the user's own mgsvtpp.exe.
# Output project goes to re/projects/ (gitignored). Takes a few hours.
set -euo pipefail
cd "$(dirname "$0")/.."
GHIDRA="${GHIDRA_HOME:-$(ls -d .tools/ghidra_*_PUBLIC | tail -1)}"
GAME="${MGSV_GAME_DIR:-..}"
export GHIDRA_HEADLESS_MAXMEM="${GHIDRA_HEADLESS_MAXMEM:-24G}"
"$GHIDRA/support/analyzeHeadless.bat" "$(pwd -W)/re/projects" mgsvtpp \
  -import "$GAME/mgsvtpp.exe" -overwrite \
  -scriptPath "$(pwd -W)/re/ghidra" \
  -preScript PrepareMgsv.java \
  -max-cpu "${MAX_CPU:-24}" \
  -analysisTimeoutPerFile 86400
