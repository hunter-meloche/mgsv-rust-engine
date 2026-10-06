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
; Tempo of all of Snake's motion in percent of MGSV's: animations, and so
; his speed (MGSV: run 2.35 m/s, crouch 1.6, crawl 0.7), turns, stance
; changes and reloads. The fire rate is unchanged.
MoveSpeedScale=150
; Gunshot volume in percent.
ShotVolume=70
; Footsteps, suit and weapon rattle, dive landing (MGSV's own sounds), percent.
FoleyVolume=100
; Footsteps and hands on the ground while crawling, percent of FoleyVolume.
FootstepVolume=100
; Suit and weapon rattle (with each step, raising and switching weapons),
; percent of FoleyVolume.
RattleVolume=25
; MGSV's camera in Snake mode (Skyrim's [Camera] settings, game units,
; restored when Snake mode ends; AimCamera=0 keeps Skyrim's camera).
; Fixed distance behind Snake (no wheel zoom), side and height offsets:
AimCamera=1
CameraDistance=180
CameraX=35
CameraZ=12
; Aiming, close over the right shoulder: side, height, and how far the
; camera pulls in from CameraDistance (positive = closer).
AimCameraX=50
AimCameraZ=-10
AimCameraAddY=135
; Reloads never run out (1), or use the weapon's reserve as in MGSV (0).
UnlimitedMagazines=1
; Dash (MGSV's sprint) while this key is held: a Windows virtual-key code,
; default 164 = Left Alt (Skyrim's sprint key; 160 = Left Shift).
SprintKey=164
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
