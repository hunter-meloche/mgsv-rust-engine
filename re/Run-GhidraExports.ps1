# Run export scripts against the analysed project (see Run-GhidraAnalysis.ps1).
#
# Usage: powershell -ExecutionPolicy Bypass -File re\Run-GhidraExports.ps1 [Script.java ...]
# With no arguments, runs ExportFunctions and ExportLuaBindings, then pairs
# the binding events into re\api\lua_bindings.json.
[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(ValueFromRemainingArguments)] [string[]]$Scripts,
    [string]$GhidraHome = $env:GHIDRA_HOME,
    [string]$MaxMem = '24G'
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
if (-not $GhidraHome) {
    $GhidraHome = Get-ChildItem "$Root\.tools" -Directory -Filter 'ghidra_*_PUBLIC' |
        Sort-Object Name | Select-Object -Last 1 -ExpandProperty FullName
}
if (-not $GhidraHome) { throw 'Ghidra not found: install it under .tools\ or set GHIDRA_HOME.' }
if (-not $Scripts) { $Scripts = @('ExportFunctions.java', 'ExportLuaBindings.java') }

Push-Location $Root
try {
    python tools\lua_bindings.py modules
    if ($LASTEXITCODE) { throw 'lua_bindings.py modules failed' }

    $out = "$Root\re\out"
    $post = @()
    foreach ($s in $Scripts) {
        switch ($s) {
            'ExportFunctions.java' { $post += '-postScript', $s, "$out\functions.csv" }
            'ExportLuaBindings.java' {
                $post += '-postScript', $s, "$out\lua_modules.txt", "$out\lua_bindings.jsonl"
                $post += '-postScript', $s, "$out\lua_members.txt", "$out\lua_members.jsonl"
            }
            'DecompileFunctions.java' { $post += '-postScript', $s, "$out\decompile_targets.txt", "$out\decomp" }
            default { $post += '-postScript', $s }
        }
    }

    $env:GHIDRA_HEADLESS_MAXMEM = $MaxMem
    & "$GhidraHome\support\analyzeHeadless.bat" "$Root\re\projects" mgsvtpp `
        -process mgsvtpp.exe -noanalysis -readOnly `
        -scriptPath "$Root\re\ghidra" @post *>&1 | ForEach-Object { "$_" } | Tee-Object -Variable log
    $log | Set-Content -Encoding UTF8 "$out\ghidra_exports.log"
    if ($LASTEXITCODE) { throw "Ghidra exited with $LASTEXITCODE" }

    if ($Scripts -contains 'ExportLuaBindings.java') {
        python tools\lua_bindings.py pair
    }
}
finally {
    Pop-Location
}
