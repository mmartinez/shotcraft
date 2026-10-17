<#
.SYNOPSIS
  Build ShotCraft and package it for Windows.

.DESCRIPTION
  Produces, in $env:DIST (default: dist\release):
    shotcraft-<version>-windows-<arch>.msi  per-machine installer with a Start Menu shortcut
    shotcraft-<version>-windows-<arch>.zip  shotcraft.exe and the license files

  The C runtime is linked statically (+crt-static), so neither needs the Visual C++ redistributable.
  Nothing is code-signed yet, so SmartScreen warns on first run.

  Needs Rust (MSVC toolchain and the target; arm64 builds cross-compile with the MSVC ARM64 build
  tools) and WiX v5: dotnet tool install --global wix --version 5.0.2

.EXAMPLE
  pwsh packaging/windows/package.ps1 -Arch x64
  pwsh packaging/windows/package.ps1 -Arch arm64 -SkipBuild
#>
param(
  [ValidateSet('x64', 'arm64')] [string] $Arch = 'x64',
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$AppId = 'io.github.mmartinez.shotcraft'

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# The version lives in one place: [workspace.package] version in the root Cargo.toml.
$Version = $null
$inPkg = $false
foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
  if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
  if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }
# MSI ProductVersion is numeric (major.minor.build), so a pre-release tag is dropped there.
$MsiVersion = ($Version -split '-')[0]

$Target = switch ($Arch) { 'x64' { 'x86_64-pc-windows-msvc' } 'arm64' { 'aarch64-pc-windows-msvc' } }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null
Write-Output "ShotCraft $Version for Windows $Arch ($Target)"

if (-not $SkipBuild) {
  # Static CRT, scoped to the target so build scripts and proc-macros on the host are unaffected.
  $flagVar = 'CARGO_TARGET_' + ($Target.ToUpper() -replace '-', '_') + '_RUSTFLAGS'
  [Environment]::SetEnvironmentVariable($flagVar, '-C target-feature=+crt-static')
  Invoke-Native "cargo build ($Target)" { cargo build --release --locked -p shotcraft --target $Target }
}
$Exe = Join-Path $TargetDir "$Target\release\shotcraft.exe"

# The PE header must match -Arch (an x64 build can never ship labelled arm64), and the subsystem must
# be Windows GUI (2) so no console window opens with the app.
$bytes = [IO.File]::ReadAllBytes($Exe)
$pe = [BitConverter]::ToInt32($bytes, 0x3C)
$machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
$subsystem = [BitConverter]::ToUInt16($bytes, $pe + 0x5C)
$expected = switch ($Arch) { 'x64' { 0x8664 } 'arm64' { 0xAA64 } }
if ($machine -ne $expected) { throw "shotcraft.exe is for machine 0x$('{0:X}' -f $machine), expected 0x$('{0:X}' -f $expected)" }
if ($subsystem -ne 2) { throw "shotcraft.exe has PE subsystem $subsystem, expected 2 (Windows GUI)" }

$Stage = Join-Path $TargetDir "windows-package\$Arch"
$DocDir = Join-Path $Stage 'licenses'
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $DocDir | Out-Null
Copy-Item $Exe $Stage
foreach ($f in 'README.md', 'LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE', 'THIRD-PARTY-LICENSES.md', 'ATTRIBUTION.md') {
  Copy-Item (Join-Path $Root $f) $DocDir
}
Copy-Item (Join-Path $Root 'crates\doc\assets\fonts\OFL.txt') (Join-Path $DocDir 'OFL-Inter.txt')

# The installer's icon (Start Menu shortcut, Installed apps): an .ico holding the hicolor PNGs as-is,
# which Windows Vista and later read. A size of 256 is written as 0 in the directory entry.
$Ico = Join-Path $Stage 'shotcraft.ico'
$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = foreach ($s in $sizes) { , [IO.File]::ReadAllBytes((Join-Path $Root "assets\app-icon\hicolor\${s}x${s}\apps\$AppId.png")) }
$ms = New-Object IO.MemoryStream
$w = New-Object IO.BinaryWriter $ms
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $dim = [byte]($sizes[$i] % 256)
  $w.Write($dim); $w.Write($dim); $w.Write([byte]0); $w.Write([byte]0)
  $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$offset)
  $offset += $pngs[$i].Length
}
foreach ($png in $pngs) { $w.Write($png) }
$w.Flush()
[IO.File]::WriteAllBytes($Ico, $ms.ToArray())

# ---- MSI ------------------------------------------------------------------------------------------
$Msi = Join-Path $Dist "shotcraft-$Version-windows-$Arch.msi"
Invoke-Native 'wix build' {
  wix build (Join-Path $PSScriptRoot 'shotcraft.wxs') -arch $Arch `
    -d "Version=$MsiVersion" -d "BinDir=$Stage" -d "DocDir=$DocDir" -d "IconPath=$Ico" -o $Msi
}
Invoke-Native 'wix msi validate' { wix msi validate $Msi -intermediateFolder (Join-Path $Stage 'msi-validation') }
# wix writes debug symbols (.wixpdb) next to the MSI; keep them out of the release assets.
Remove-Item -Force -ErrorAction SilentlyContinue ([IO.Path]::ChangeExtension($Msi, '.wixpdb'))

# ---- zip ------------------------------------------------------------------------------------------
$ZipDir = Join-Path $TargetDir "windows-package\shotcraft-$Version-windows-$Arch"
Remove-Item -Recurse -Force $ZipDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $ZipDir | Out-Null
Copy-Item (Join-Path $Stage 'shotcraft.exe') $ZipDir
Copy-Item (Join-Path $DocDir '*') $ZipDir
$Zip = Join-Path $Dist "shotcraft-$Version-windows-$Arch.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path $ZipDir -DestinationPath $Zip

# Smoke-test the binary when this machine can run it (an arm64 build made on an x64 runner can't).
# The GUI subsystem gives it no console, so read --version through a redirected stdout.
$HostArch = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
if ($Arch -eq $HostArch) {
  $out = Join-Path $Stage 'version.txt'
  $p = Start-Process -FilePath (Join-Path $Stage 'shotcraft.exe') -ArgumentList '--version' -NoNewWindow -Wait -PassThru -RedirectStandardOutput $out
  $text = (Get-Content -Raw $out).Trim()
  if ($p.ExitCode -ne 0 -or $text -ne "shotcraft $Version") { throw "shotcraft.exe --version printed '$text' (exit $($p.ExitCode))" }
  Write-Output "ok: $text"
} else {
  Write-Output "skipping shotcraft.exe --version: an $Arch build doesn't run on this $HostArch machine"
}
Get-Item $Msi, $Zip | Format-Table Name, Length
