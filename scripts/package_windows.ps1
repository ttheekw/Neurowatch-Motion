# Build the nw-motion Windows release: an installer and portable zip with GStreamer
# (only the plugins the analysis uses, and the DLLs they need) and the MSVC
# runtime, so it runs without installing anything.
#
# Run from an MSVC developer shell with GStreamer's MSVC runtime and
# development packages installed (GSTREAMER_1_0_ROOT_MSVC_X86_64), as the
# release workflow does. Nothing is published.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$OutDir = 'dist/release',
    [string]$GStreamerRoot = $env:GSTREAMER_1_0_ROOT_MSVC_X86_64,
    [string]$InnoSetupCompiler = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Root = Split-Path -Parent $PSScriptRoot

function Invoke-Native {
    param([string]$Command, [string[]]$Arguments)
    $output = @(& $Command @Arguments 2>&1)
    $exitCode = $LASTEXITCODE
    $output | ForEach-Object { Write-Host $_ }
    if ($exitCode -ne 0) {
        $tail = ($output | Select-Object -Last 30) -join "`n"
        throw "$Command failed with exit code ${exitCode}:`n$tail"
    }
}

$Version = $Version.TrimStart('v')
if ($Version -notmatch '^\d+\.\d+\.\d+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?$') {
    throw "Version must look like 1.2.3 or 1.2.3-rc.1, got: $Version"
}
$CargoVersion = (Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"(.*)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if ($Version.Split('-')[0] -ne $CargoVersion) {
    throw "Version $Version does not match Cargo.toml ($CargoVersion)"
}
if (-not $GStreamerRoot -or -not (Test-Path (Join-Path $GStreamerRoot 'bin/gstreamer-1.0-0.dll'))) {
    throw 'GStreamer MSVC runtime not found: set GSTREAMER_1_0_ROOT_MSVC_X86_64'
}
if (-not $env:VCToolsRedistDir) { throw 'Run from an MSVC developer shell (VCToolsRedistDir missing)' }
if (-not (Test-Path $InnoSetupCompiler)) { throw "Inno Setup compiler not found: $InnoSetupCompiler" }

$WorkDir = Join-Path $Root 'build/release-windows-x86_64'
$BundleDir = Join-Path $WorkDir 'bundle/nw-motion'
$Bin = Join-Path $BundleDir 'bin'
$Plugins = Join-Path $BundleDir 'gstreamer/lib/gstreamer-1.0'
if (Test-Path $WorkDir) { Remove-Item -Recurse -Force $WorkDir }
New-Item -ItemType Directory -Force -Path $Bin, $Plugins, $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

Write-Host '[1/5] Building nw-motion (release)'
$env:PKG_CONFIG_PATH = Join-Path $GStreamerRoot 'lib/pkgconfig'
Invoke-Native 'cargo' @('build', '--manifest-path', (Join-Path $Root 'Cargo.toml'), '--release', '--locked')
$Target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
Copy-Item (Join-Path $Target 'release/nw-motion.exe') $Bin
Invoke-Native 'rustc' @('--edition=2021', '-C', 'opt-level=s', '-C', 'strip=symbols',
    (Join-Path $PSScriptRoot 'packaging/windows-launcher.rs'), '-o', (Join-Path $Bin 'nw-motion-launcher.exe'))
foreach ($file in @('LICENSE', 'NOTICE', 'README.md')) {
    Copy-Item (Join-Path $Root $file) $BundleDir
}

Write-Host '[2/5] Bundling GStreamer and the MSVC runtime'
# The plugins the analysis pipelines use (src/worker.rs), then the hardware
# decoders --hardware-decoding may pick, when this GStreamer has them.
$Required = @('coreelements', 'app', 'playback', 'typefindfunctions',
    'videoconvertscale', 'videoparsersbad', 'libav')
$Optional = @('d3d11', 'd3d12', 'nvcodec', 'qsv')
foreach ($name in $Required) {
    $dll = Join-Path $GStreamerRoot "lib/gstreamer-1.0/gst$name.dll"
    if (-not (Test-Path $dll)) { throw "GStreamer lacks the $name plugin" }
    Copy-Item $dll $Plugins
}
foreach ($name in $Optional) {
    $dll = Join-Path $GStreamerRoot "lib/gstreamer-1.0/gst$name.dll"
    if (Test-Path $dll) { Copy-Item $dll $Plugins } else { Write-Host "  (no $name plugin: left out)" }
}
# The runtime DLLs they need, following their own needs. Beside the program:
# Windows finds a DLL in the program's folder, plugins' needs included.
$RuntimeBin = Join-Path $GStreamerRoot 'bin'
$Queue = [System.Collections.Generic.Queue[string]]::new()
Get-ChildItem $Bin, $Plugins -File | Where-Object { $_.Extension -in '.exe', '.dll' } |
    ForEach-Object { $Queue.Enqueue($_.FullName) }
while ($Queue.Count -gt 0) {
    $file = $Queue.Dequeue()
    $dependents = & dumpbin /nologo /dependents $file
    if ($LASTEXITCODE -ne 0) { throw "dumpbin failed on $file" }
    foreach ($dll in ($dependents | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^[^\\/:\s]+\.dll$' })) {
        $source = Join-Path $RuntimeBin $dll
        $copy = Join-Path $Bin $dll
        if ((Test-Path $source) -and -not (Test-Path $copy)) {
            Copy-Item $source $copy
            $Queue.Enqueue($copy)
        }
    }
}
# The LGPL and other media licences must ship: a layout without them fails.
$Licenses = @('share/licenses', 'share/gstreamer-1.0/licenses') |
    ForEach-Object { Join-Path $GStreamerRoot $_ } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $Licenses) { throw "GStreamer licences not found under $GStreamerRoot" }
Copy-Item -Recurse -Force $Licenses (Join-Path $BundleDir 'gstreamer/licenses')
# App-local MSVC runtime: no system-wide installer or administrator rights.
$Crt = Get-ChildItem (Join-Path $env:VCToolsRedistDir 'x64/Microsoft.VC*.CRT') -Directory | Select-Object -First 1
if (-not $Crt) { throw 'MSVC x64 redistributable runtime missing' }
Copy-Item (Join-Path $Crt.FullName '*.dll') $Bin -Force

Write-Host '[3/5] Checking the bundle without the GStreamer install'
$SavedPath = $env:PATH
$SavedLocal = $env:LOCALAPPDATA
try {
    $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
    $env:LOCALAPPDATA = Join-Path $WorkDir 'check-cache'
    foreach ($name in @('GST_PLUGIN_PATH', 'GST_PLUGIN_PATH_1_0', 'GST_PLUGIN_SYSTEM_PATH',
            'GST_PLUGIN_SYSTEM_PATH_1_0', 'GST_REGISTRY', 'GST_REGISTRY_1_0')) {
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    Invoke-Native (Join-Path $Bin 'nw-motion.exe') @('--check')
} finally {
    $env:PATH = $SavedPath
    $env:LOCALAPPDATA = $SavedLocal
}

Write-Host '[4/5] Zip'
$Zip = Join-Path $OutDir "nw-motion-$Version-windows-x86_64.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path $BundleDir -DestinationPath $Zip
Write-Host "  zip: $Zip"

Write-Host '[5/5] Installer (starts locally now and at sign-in)'
Invoke-Native $InnoSetupCompiler @("/DVersion=$Version", "/DBundleDir=$BundleDir", "/DOutputDir=$OutDir",
    (Join-Path $PSScriptRoot 'packaging/windows-installer.iss'))
