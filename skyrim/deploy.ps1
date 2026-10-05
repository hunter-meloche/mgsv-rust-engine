# Copy SnakeSKSE.dll + snake.dll into Skyrim's Data/SKSE/Plugins, write
# snake.ini pointing at this MGSV install, and export Snake's weapon meshes
# and textures from your MGSV install into Data/meshes/snake and
# Data/textures/snake. Remove those files and folders to undo.
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
; Damage of each bullet (Skyrim scale: a Hunting Bow does 7, a Daedric Bow 19).
BulletDamage=48
; Snake's movement speed in percent of MGSV's (run 2.35 m/s, crouch 1.6,
; crawl 0.7); Skyrim's own run is about 5 m/s.
MoveSpeedScale=100
; Gunshot volume in percent.
ShotVolume=70
; Over-the-shoulder aim camera in Snake mode, as in MGSV (Skyrim's
; fOverShoulderCombatPosX/PosZ while aiming, game units; restored when
; Snake mode ends). AimCamera=0 keeps Skyrim's camera.
AimCamera=1
AimCameraX=120
AimCameraZ=0
; Reloads never run out (1), or use the weapon's reserve as in MGSV (0).
UnlimitedMagazines=1
; Hidden Sneak skill bonus while prone (crawling).
CrawlSneakBonus=20
"@
Set-Content -Path (Join-Path $plugins "snake.ini") -Value $ini -Encoding ASCII

$mgsvExe = Join-Path $repo "target\release\mgsv.exe"
if (Test-Path $mgsvExe) {
    & $mgsvExe skyrim-export --skyrim $Skyrim
    if ($LASTEXITCODE -ne 0) { throw "weapon export failed" }
} else {
    Write-Warning "target\release\mgsv.exe not built; skipping the weapon mesh export (cargo build --release -p mgsv-cli)"
}
Get-ChildItem $plugins | Where-Object { $_.Name -in @("SnakeSKSE.dll", "snake.dll", "snake.ini") } | Format-Table Name, Length, LastWriteTime
