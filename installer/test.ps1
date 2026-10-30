# Installs the setup silently per-user, checks what it put in place, starts
# the installed app, uninstalls silently and checks everything is gone.
param([string]$out = "results")
$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force $out | Out-Null
$v = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$setup = "target\installer\RengiSnap-Setup-$v.exe"
$log = "$out\installer.txt"
Set-Content $log "installer test $v"
$script:fail = 0
function Check($name, $ok, $detail = '') {
    $line = "{0} {1} {2}" -f ($(if ($ok) { 'PASS' } else { 'FAIL' }), $name, $detail)
    Add-Content $log $line
    Write-Host $line
    if (-not $ok) { $script:fail++ }
}
function WaitFor([scriptblock]$cond, [int]$seconds = 20) {
    for ($i = 0; $i -lt $seconds * 4; $i++) { if (& $cond) { return $true }; Start-Sleep -Milliseconds 250 }
    return $false
}

Get-Process RengiSnap -ErrorAction SilentlyContinue | Stop-Process -Force
$p = Start-Process $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/TASKS="autostart,openwith,desktopicon"', "/LOG=$out\setup-log.txt" -Wait -PassThru
Check "setup exits 0" ($p.ExitCode -eq 0) $p.ExitCode
$dir = "$env:LOCALAPPDATA\Programs\Rengi Snap"
$exe = "$dir\RengiSnap.exe"
Check "exe installed per-user" (Test-Path $exe) $exe
$lnk = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Snap by rng.lnk"
Check "Start menu shortcut" (Test-Path $lnk) $lnk
$desk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Snap by rng.lnk'
Check "desktop shortcut" (Test-Path $desk) $desk
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$run = (Get-ItemProperty $runKey -Name RengiSnap -ErrorAction SilentlyContinue).RengiSnap
Check "autostart value points at the installed exe" ($run -eq "`"$exe`" --background") $run
$cmd = (Get-ItemProperty 'HKCU:\Software\Classes\Applications\RengiSnap.exe\shell\open\command' -ErrorAction SilentlyContinue).'(default)'
Check "Open with command" ($cmd -eq "`"$exe`" --edit `"%1`"") $cmd
Check "mp4 and png list Snap in Open with" ((Test-Path 'HKCU:\Software\Classes\.mp4\OpenWithList\RengiSnap.exe') -and (Test-Path 'HKCU:\Software\Classes\.png\OpenWithList\RengiSnap.exe'))
$ver = (Get-Item $exe).VersionInfo
Check "exe has version info" ($ver.FileDescription -eq 'Snap by rng' -and $ver.ProductVersion -eq $v) "$($ver.FileDescription) / $($ver.ProductVersion)"
Add-Type -AssemblyName System.Drawing
$ico = [System.Drawing.Icon]::ExtractAssociatedIcon($exe)
$bmp = $ico.ToBitmap()
$c = $bmp.GetPixel([int]($bmp.Width / 2), [int]($bmp.Height / 6))
Check "exe icon is the blue squircle" ($c.B -gt 200 -and $c.R -lt 160 -and $c.A -gt 200) "$c"
$bmp.Save("$out\installed-icon.png")

# The installed app starts like a user would start it (Home opens).
$env:RENGI_SNAP_HOME = (Resolve-Path $out).Path + "\inst-home"
New-Item -ItemType Directory -Force $env:RENGI_SNAP_HOME | Out-Null
# An older copy from elsewhere is already running in the tray (like a
# download from before): the installed app must replace it, not hand over to it.
$old = "$out\old-copy"
New-Item -ItemType Directory -Force $old | Out-Null
Copy-Item target\release\RengiSnap.exe "$old\RengiSnap.exe"
Start-Process "$old\RengiSnap.exe" -ArgumentList '--background'
WaitFor { $null -ne (Get-Process RengiSnap -ErrorAction SilentlyContinue) } 10 | Out-Null
Start-Sleep 1
Start-Process $exe
Check "installed app runs" (WaitFor { $null -ne (Get-Process RengiSnap -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) } 10)
Check "installed app replaced the older copy" (WaitFor { $null -eq (Get-Process RengiSnap -ErrorAction SilentlyContinue | Where-Object { $_.Path -ne $exe }) } 10) ((Get-Process RengiSnap -ErrorAction SilentlyContinue | ForEach-Object { $_.Path }) -join ', ')
Start-Sleep 2
$runAfter = (Get-ItemProperty $runKey -Name RengiSnap -ErrorAction SilentlyContinue).RengiSnap
Check "first run keeps the installer's autostart choice" ($runAfter -eq $run) $runAfter

# Uninstall: the uninstaller re-launches itself from %TEMP%, so wait for the result.
$unins = "$dir\unins000.exe"
Check "uninstaller present" (Test-Path $unins)
$p = Start-Process $unins -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
Check "uninstaller starts" ($p.ExitCode -eq 0) $p.ExitCode
Check "exe removed" (WaitFor { -not (Test-Path $exe) } 30)
Check "app stopped by uninstall" (WaitFor { $null -eq (Get-Process RengiSnap -ErrorAction SilentlyContinue) } 10)
Check "shortcuts removed" ((-not (Test-Path $lnk)) -and (-not (Test-Path $desk)))
Check "autostart removed" (WaitFor { $null -eq (Get-ItemProperty $runKey -Name RengiSnap -ErrorAction SilentlyContinue) } 10)
Check "Open with removed" (-not (Test-Path 'HKCU:\Software\Classes\Applications\RengiSnap.exe'))

$zip = "target\installer\RengiSnap-$v-portable.zip"
Add-Type -AssemblyName System.IO.Compression.FileSystem
$entries = @()
if (Test-Path $zip) { $z = [IO.Compression.ZipFile]::OpenRead((Resolve-Path $zip).Path); $entries = $z.Entries.FullName; $z.Dispose() }
Check "portable zip has the exe" ($entries -contains 'RengiSnap.exe') ($entries -join ', ')

Add-Content $log "installer failures: $script:fail"
if ($script:fail -ne 0) { exit 1 }
exit 0
