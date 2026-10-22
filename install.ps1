# Installs HatchZip on Windows from the latest release, or updates it.
#
#   irm https://raw.githubusercontent.com/xevrion/hatchzip/main/install.ps1 | iex
#
# It downloads the Windows build, checks it against the published checksum,
# puts it in %LOCALAPPDATA%\Programs\HatchZip and adds that folder to your
# PATH. No administrator rights needed.
#
# To remove it again:
#
#   $env:HATCHZIP_UNINSTALL = 1; irm https://raw.githubusercontent.com/xevrion/hatchzip/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo = 'xevrion/hatchzip'
$Dir = Join-Path $env:LOCALAPPDATA 'Programs\HatchZip'

function Get-UserPath {
    [Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { $_ }
}

if ($env:HATCHZIP_UNINSTALL) {
    Remove-Item Env:\HATCHZIP_UNINSTALL
    Write-Host 'Removing HatchZip' -ForegroundColor White
    Remove-Item -Recurse -Force $Dir -ErrorAction SilentlyContinue
    $kept = Get-UserPath | Where-Object { $_ -ne $Dir }
    [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
    Write-Host 'Removed. Half-finished extractions keep their .hatchzip files next to the archive.'
    return
}

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64' -and $arch -ne 'ARM64') {
    throw "HatchZip needs 64-bit Windows, this is $arch."
}
# ARM64 Windows runs the x64 build through its built-in emulation.
$name = 'hatchzip-x86_64-windows'
$base = "https://github.com/$Repo/releases/latest/download"
if ($env:HATCHZIP_VERSION) {
    $base = "https://github.com/$Repo/releases/download/$env:HATCHZIP_VERSION"
}
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("hatchzip-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Write-Host 'Downloading HatchZip for Windows' -ForegroundColor White
    $zip = Join-Path $tmp "$name.zip"
    $sum = Join-Path $tmp "$name.zip.sha256"
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$name.zip" -OutFile $zip
    # Saved to a file and read back as text: GitHub serves it as binary, so
    # reading the response directly gives bytes, not the hash.
    Invoke-WebRequest -UseBasicParsing -Uri "$base/$name.zip.sha256" -OutFile $sum
    $expected = ((Get-Content -Raw $sum).Trim() -split '\s+')[0]
    $actual = (Get-FileHash -Algorithm SHA256 $zip).Hash
    if ($actual -ne $expected.ToUpper()) {
        throw 'The download does not match its checksum, nothing was installed.'
    }

    Write-Host "Installing to $Dir" -ForegroundColor White
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Expand-Archive -Force -Path $zip -DestinationPath $Dir
    Get-ChildItem $Dir | Unblock-File

    if ((Get-UserPath) -notcontains $Dir) {
        $paths = @(Get-UserPath) + $Dir
        [Environment]::SetEnvironmentVariable('Path', ($paths -join ';'), 'User')
        $env:Path = "$env:Path;$Dir"
        Write-Host "Added $Dir to your PATH. New terminals pick it up automatically."
    }

    # `hz` is the short name, unless some other program already answers to it.
    $otherHz = Get-Command hz -ErrorAction SilentlyContinue | Where-Object { $_.Source -ne (Join-Path $Dir 'hz.exe') }
    if ($otherHz) {
        Write-Host "Skipping the short name hz: $($otherHz.Source) already uses it."
    } else {
        Copy-Item -Force (Join-Path $Dir 'hatchzip.exe') (Join-Path $Dir 'hz.exe')
    }

    $version = & (Join-Path $Dir 'hatchzip.exe') --version
    Write-Host ''
    Write-Host "$version installed." -ForegroundColor Green
    Write-Host 'Try: hatchzip check some-archive.zip (or the short name: hz check some-archive.zip)'
    Write-Host "Tab completion: add '. $Dir\hatchzip-completion.ps1' to your `$PROFILE."
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
