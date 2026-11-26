# Builds Gamut and installs it to %LOCALAPPDATA%\Programs\Gamut with Desktop + Start Menu shortcuts.
# Re-run after pulling changes to update. Usage: powershell -ExecutionPolicy Bypass -File .\install.ps1
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$dir = Join-Path $env:LOCALAPPDATA 'Programs\Gamut'
New-Item -ItemType Directory -Force $dir | Out-Null
if (Get-Process gamut -ErrorAction SilentlyContinue) { throw "Gamut is running - close it and run this again." }
# Honor CARGO_TARGET_DIR, else a stale target\release exe gets installed.
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
Copy-Item (Join-Path $target 'release\gamut.exe') (Join-Path $dir 'gamut.exe') -Force

$shell = New-Object -ComObject WScript.Shell
$targets = @(
    [Environment]::GetFolderPath('Desktop'),
    (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs')
)
foreach ($t in $targets) {
    $lnk = $shell.CreateShortcut((Join-Path $t 'Gamut.lnk'))
    $lnk.TargetPath = Join-Path $dir 'gamut.exe'
    $lnk.WorkingDirectory = $dir
    $lnk.Description = 'Gamut game library'
    $lnk.Save()
}
Write-Host "Installed to $dir - shortcuts added to Desktop and Start Menu."
