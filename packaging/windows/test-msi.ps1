# Runs the MSI the way an administrator or winget would, silently: install,
# check what it left, run the installed CLI, uninstall, check that nothing is
# left. Given the setup too, it also checks that neither installs over the
# other. Used by the release workflow before anything is published.
#
#   packaging\windows\test-msi.ps1 dist\HatchZip-x64.msi [dist\HatchZip-x64-setup.exe]

param(
    [Parameter(Mandatory)] [string] $Msi,
    [string] $Setup
)

$ErrorActionPreference = 'Stop'
$Msi = (Resolve-Path $Msi).Path
$dir = Join-Path $env:LOCALAPPDATA 'Programs\HatchZip'
$link = Join-Path ([Environment]::GetFolderPath('Programs')) 'HatchZip.lnk'
$uninstall = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
# Written by `hatchzip-gui --register-shell` (crates/shell).
$menuKey = 'HKCU:\Software\Classes\SystemFileAssociations\.zip\shell\HatchZip'
$progId = 'HKCU:\Software\Classes\HatchZip.Archive'
$appKey = 'HKCU:\Software\Classes\Applications\hatchzip-gui.exe'
$extensions = 'zip', '7z', 'tar', 'tgz', 'gz', '001', 'rar', 'r00'
$log = Join-Path $env:RUNNER_TEMP 'msi.log'
if (-not $env:RUNNER_TEMP) { $log = Join-Path $env:TEMP 'hatchzip-msi.log' }

# Runs msiexec and returns its exit code. The log is printed when the code
# isn't the one expected.
function Msiexec($arguments, [int] $expect = 0) {
    $p = Start-Process -FilePath msiexec.exe -ArgumentList ($arguments + '/qn', '/norestart', '/l*v', "`"$log`"") -PassThru
    $null = $p.Handle # keeps the exit code readable after it exits
    if (-not $p.WaitForExit(300000)) { throw "msiexec $arguments is still running after 5 minutes" }
    if ($p.ExitCode -ne $expect) {
        Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 80 | Out-Host
        throw "msiexec $arguments exited with $($p.ExitCode), expected $expect"
    }
}

function RunSetup($arguments) {
    $p = Start-Process -FilePath $Setup -ArgumentList $arguments -PassThru
    $null = $p.Handle
    if (-not $p.WaitForExit(300000)) { throw "$Setup is still running after 5 minutes" }
    $p.ExitCode
}

function UserPath {
    [Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ }
}

function PathKind {
    $key = Get-Item 'HKCU:\Environment'
    if ($key.GetValueNames() -contains 'Path') { $key.GetValueKind('Path') } else { 'none' }
}

# The MSI's own entry in Settings > Apps, keyed by its product code.
function UninstallEntry {
    Get-ChildItem $uninstall -ErrorAction SilentlyContinue | Get-ItemProperty |
        Where-Object { $_.DisplayName -eq 'HatchZip' -and $_.WindowsInstaller -eq 1 }
}

function OpenWithHas($ext) {
    $key = "HKCU:\Software\Classes\.$ext\OpenWithProgids"
    (Test-Path $key) -and ((Get-ItemProperty $key).PSObject.Properties.Name -contains 'HatchZip.Archive')
}

$kindBefore = PathKind

Write-Host 'install'
Msiexec '/i', "`"$Msi`""
foreach ($f in 'hatchzip-gui.exe', 'hatchzip.exe', 'hz.exe', 'hatchzip-completion.ps1', 'README.md',
        'LICENSE-MIT.txt', 'LICENSE-APACHE.txt', 'LICENSE-UNRAR.txt') {
    if (-not (Test-Path (Join-Path $dir $f))) { throw "$f is missing" }
}
if (-not (Test-Path $link)) { throw 'the Start menu shortcut is missing' }
$target = (New-Object -ComObject WScript.Shell).CreateShortcut($link).TargetPath
if ($target -ne (Join-Path $dir 'hatchzip-gui.exe')) { throw "the Start menu shortcut points at $target" }
$entry = @(UninstallEntry)
if ($entry.Count -ne 1) { throw "expected one HatchZip MSI entry in Apps and features, found $($entry.Count)" }
Write-Host "Apps and features: $($entry.DisplayName) $($entry.DisplayVersion), $($entry.Publisher), $($entry.URLInfoAbout)"
if ((UserPath) -notcontains $dir) { throw "$dir is not on the user PATH: $([Environment]::GetEnvironmentVariable('Path', 'User'))" }
if ((PathKind) -ne $kindBefore) { throw "the user PATH changed from $kindBefore to $(PathKind)" }
if (-not (Test-Path $menuKey)) { throw 'the right-click menu was not registered' }
foreach ($ext in $extensions) {
    if (-not (OpenWithHas $ext)) { throw "HatchZip is not in Open with for .$ext" }
}
$command = (Get-ItemProperty "$progId\shell\open\command").'(default)'
if ($command -ne "`"$dir\hatchzip-gui.exe`" `"%1`"") { throw "the Open with command is $command" }
if ((Get-ItemProperty $appKey).FriendlyAppName -ne 'HatchZip') { throw 'FriendlyAppName is not set' }

$version = & (Join-Path $dir 'hatchzip.exe') --version
if ($LASTEXITCODE) { throw 'the installed hatchzip.exe does not run' }
Write-Host $version
& (Join-Path $dir 'hz.exe') --version | Out-Host
if ($LASTEXITCODE) { throw 'the installed hz.exe does not run' }
# Windows Installer keeps three numbers: 0.2.0-beta.1 is shown as 0.2.0.
if ($entry.DisplayVersion -ne (($version -split ' ')[1] -split '[-+]')[0]) { throw 'Apps and features shows a different version from the program' }
python .github/scripts/smoke.py (Join-Path $dir 'hatchzip.exe')
if ($LASTEXITCODE) { throw 'the smoke test failed on the installed hatchzip.exe' }

if ($Setup) {
    Write-Host 'the setup refuses to install over the MSI'
    $code = RunSetup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    if ($code -eq 0) { throw 'the setup installed over the MSI' }
    if (Test-Path (Join-Path $dir 'unins000.exe')) { throw 'the setup left its uninstaller in the MSI install' }
}

Write-Host 'uninstall'
Msiexec '/x', "`"$Msi`""
if (Test-Path $dir) { throw "the install folder is still there: $((Get-ChildItem $dir -Recurse).FullName -join ', ')" }
if (Test-Path $link) { throw 'the Start menu shortcut is still there' }
if (@(UninstallEntry).Count) { throw 'it is still in Apps and features' }
if ((UserPath) -contains $dir) { throw "$dir is still on the user PATH" }
if ((PathKind) -ne $kindBefore) { throw "the user PATH changed from $kindBefore to $(PathKind)" }
if (Test-Path $menuKey) { throw 'the right-click menu is still there' }
foreach ($ext in $extensions) {
    if (OpenWithHas $ext) { throw "HatchZip is still in Open with for .$ext" }
}
if (Test-Path $progId) { throw "$progId is still there" }
if (Test-Path $appKey) { throw "$appKey is still there" }

if ($Setup) {
    Write-Host 'the MSI refuses to install over the setup'
    $code = RunSetup '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART'
    if ($code) { throw "the setup exited with $code" }
    # 1603: the launch condition stopped it.
    Msiexec '/i', "`"$Msi`"" 1603
    if (@(UninstallEntry).Count) { throw 'the MSI installed over the setup' }
    $p =Start-Process -FilePath (Join-Path $dir 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -PassThru
    $null = $p.Handle
    if (-not $p.WaitForExit(300000) -or $p.ExitCode) { throw 'uninstalling the setup failed' }
    foreach ($i in 1..30) {
        if (-not (Test-Path $dir)) { break }
        Start-Sleep -Seconds 1
    }
    if (Test-Path $dir) { throw 'the setup left its install folder' }
}
Write-Host 'the MSI installs and uninstalls cleanly'
