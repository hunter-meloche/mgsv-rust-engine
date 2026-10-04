# Copy SnakeSKSE.dll + snake.dll into Skyrim's Data/SKSE/Plugins and write
# snake.ini pointing at this MGSV install. Remove those three files to undo.
#
#   pwsh skyrim/deploy.ps1 -Skyrim "H:\SteamLibrary\steamapps\common\Skyrim Special Edition"
param(
    [Parameter(Mandatory = $true)][string]$Skyrim
)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$plugins = Join-Path $Skyrim "Data\SKSE\Plugins"
New-Item -ItemType Directory -Force $plugins | Out-Null

$files = @(
    (Join-Path $repo "skyrim\snake-skse\build\Release\SnakeSKSE.dll"),
    (Join-Path $repo "target\release\snake.dll")
)
foreach ($f in $files) {
    if (-not (Test-Path $f)) { throw "missing $f (build it first; see skyrim/README.md)" }
    Copy-Item $f $plugins -Force
}

$mgsv = Split-Path -Parent $repo
$ini = @"
[Snake]
; Your MGSV: TPP install (the folder with mgsvtpp.exe).
MgsvDir=$mgsv
; This repo's dictionaries folder.
DictionariesDir=$(Join-Path $repo "dictionaries")
"@
Set-Content -Path (Join-Path $plugins "snake.ini") -Value $ini -Encoding ASCII
Get-ChildItem $plugins | Where-Object { $_.Name -in @("SnakeSKSE.dll", "snake.dll", "snake.ini") } | Format-Table Name, Length, LastWriteTime
