# Builds the setup and the portable zip into target\installer.
$ErrorActionPreference = 'Stop'
$v = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) {
    $c = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    if (Test-Path $c) { $iscc = $c }
}
if (-not $iscc) {
    choco install innosetup -y --no-progress | Out-Null
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
}
Write-Host "ISCC: $iscc, version $v"
& $iscc "/DAppVersion=$v" installer\Snap.iss
if ($LASTEXITCODE -ne 0) { exit 1 }

$z = "target\installer\portable"
New-Item -ItemType Directory -Force $z | Out-Null
Copy-Item target\release\Snap.exe "$z\Snap.exe"
@"
Snap $v - portable

Run Snap.exe. It lives in the tray; press Scroll Lock to capture.
Settings are kept in %APPDATA%\Snap. Nothing else is installed.
For a Start menu entry, autostart and "Open with", use Snap-Setup-$v.exe instead.

https://github.com/MRRengi/rng-snap
"@ | Set-Content -Encoding UTF8 "$z\Read me.txt"
Compress-Archive -Path "$z\*" -DestinationPath "target\installer\Snap-$v-portable.zip" -Force
Get-ChildItem target\installer -File | ForEach-Object { Write-Host "$($_.Name) $($_.Length) bytes" }
