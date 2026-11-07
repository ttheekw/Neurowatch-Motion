param([Parameter(Mandatory = $true)][string]$Installer)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$Installer = (Resolve-Path $Installer).Path
$InstallDir = Join-Path $env:RUNNER_TEMP 'motion installer with spaces'
$Exe = Join-Path $InstallDir 'bin/nw-motion.exe'
$Shortcut = Join-Path ([Environment]::GetFolderPath('Startup')) 'NeuroWatch Motion.lnk'
$OriginalLocalAppData = $env:LOCALAPPDATA
$env:LOCALAPPDATA = Join-Path $env:RUNNER_TEMP 'motion-installer-state'
$Identity = Join-Path $env:LOCALAPPDATA 'neurowatch/plugins/nw-motion.key'

function Wait-Daemon {
    param([bool]$Running)
    foreach ($i in 1..50) {
        $processes = @(Get-CimInstance Win32_Process -Filter "Name='nw-motion.exe'" |
            Where-Object { $_.ExecutablePath -eq $Exe })
        if (($processes.Count -gt 0) -eq $Running) { return $processes }
        Start-Sleep -Milliseconds 200
    }
    throw "Expected installed daemon running=$Running"
}

function Install-Daemon {
    $log = Join-Path $env:RUNNER_TEMP 'motion-install.log'
    $process = Start-Process $Installer -Wait -PassThru -ArgumentList @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
        "/DIR=`"$InstallDir`"", "/LOG=`"$log`"")
    if ($process.ExitCode -ne 0) { Get-Content $log; throw "Installer failed: $($process.ExitCode)" }
    Wait-Daemon $true | Out-Null
    # This is the default local discovery pipe, without any --listen argument.
    $pipe = [IO.Pipes.NamedPipeClientStream]::new('.', 'neurowatch-motion', [IO.Pipes.PipeDirection]::InOut)
    try { $pipe.Connect(10000) } finally { $pipe.Dispose() }
}

try {
    Install-Daemon
    $shell = New-Object -ComObject WScript.Shell
    if (-not (Test-Path $Shortcut)) { throw 'Sign-in shortcut was not installed' }
    if ($shell.CreateShortcut($Shortcut).TargetPath -ne (Join-Path $InstallDir 'bin/nw-motion-launcher.exe')) {
        throw 'Sign-in shortcut does not target the background launcher'
    }
    $before = (Get-FileHash $Identity -Algorithm SHA256).Hash
    Install-Daemon
    if ((Get-FileHash $Identity -Algorithm SHA256).Hash -ne $before) { throw 'Upgrade replaced the persistent identity' }
    Write-Host 'Windows installer: local startup, sign-in shortcut and upgrade passed'
} finally {
    try {
        $uninstaller = Join-Path $InstallDir 'unins000.exe'
        if (Test-Path $uninstaller) {
            $process = Start-Process $uninstaller -Wait -PassThru -ArgumentList @(
                '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART')
            if ($process.ExitCode -ne 0) { throw "Uninstaller failed: $($process.ExitCode)" }
            Wait-Daemon $false | Out-Null
            if (Test-Path $Shortcut) { throw 'Sign-in shortcut remains after uninstall' }
            if (-not (Test-Path $Identity)) { throw 'Uninstall deleted the persistent identity' }
            Write-Host 'Windows installer: uninstall stopped the daemon and removed startup'
        }
    } finally {
        $env:LOCALAPPDATA = $OriginalLocalAppData
    }
}
