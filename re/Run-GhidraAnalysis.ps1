# One-time Ghidra import and auto-analysis of your own mgsvtpp.exe.
# The project is written to re\projects\ (gitignored). Takes ~15 minutes on a
# fast machine. Re-running replaces the existing analysis.
#
# Usage: powershell -ExecutionPolicy Bypass -File re\Run-GhidraAnalysis.ps1
[CmdletBinding(PositionalBinding = $false)]
param(
    [string]$GameDir = $env:MGSV_GAME_DIR,
    [string]$GhidraHome = $env:GHIDRA_HOME,
    [string]$MaxMem = '24G',
    [int]$MaxCpu = [Math]::Max(1, [Environment]::ProcessorCount - 4)
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
if (-not $GameDir) { $GameDir = Split-Path -Parent $Root }
if (-not $GhidraHome) {
    $GhidraHome = Get-ChildItem "$Root\.tools" -Directory -Filter 'ghidra_*_PUBLIC' |
        Sort-Object Name | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $GhidraHome) { throw 'Ghidra not found: install it under .tools\ or set GHIDRA_HOME.' }
$Exe = Join-Path $GameDir 'mgsvtpp.exe'
if (-not (Test-Path $Exe)) { throw "mgsvtpp.exe not found in $GameDir" }

New-Item -ItemType Directory -Force "$Root\re\projects", "$Root\re\out" | Out-Null
$env:GHIDRA_HEADLESS_MAXMEM = $MaxMem

& "$GhidraHome\support\analyzeHeadless.bat" "$Root\re\projects" mgsvtpp `
    -import $Exe -overwrite `
    -scriptPath "$Root\re\ghidra" `
    -preScript PrepareMgsv.java `
    -max-cpu $MaxCpu `
    -analysisTimeoutPerFile 172800 *>&1 | ForEach-Object { "$_" } | Tee-Object -Variable log
$code = $LASTEXITCODE
$log | Set-Content -Encoding UTF8 "$Root\re\out\ghidra_analysis.log"
exit $code
