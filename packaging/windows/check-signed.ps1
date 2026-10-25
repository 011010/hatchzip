# Checks that each file the signing service returned carries a valid
# Authenticode signature, then copies it over the unsigned one. Used by the
# release workflow after each signing step.
#
#   packaging\windows\check-signed.ps1 <signed folder> <destination> <file>...

param(
    [Parameter(Mandatory)] [string] $From,
    [Parameter(Mandatory)] [string] $To,
    [Parameter(Mandatory, ValueFromRemainingArguments)] [string[]] $Files
)

$ErrorActionPreference = 'Stop'
foreach ($f in $Files) {
    $path = Join-Path $From $f
    if (-not (Test-Path $path)) { throw "$f did not come back from signing" }
    $s = Get-AuthenticodeSignature $path
    if ($s.Status -ne 'Valid') { throw "$f is not validly signed: $($s.Status), $($s.StatusMessage)" }
    if (-not $s.TimeStamperCertificate) { throw "$f is signed without a timestamp, so the signature ends with the certificate" }
    Write-Host "$f signed by $($s.SignerCertificate.Subject), timestamped by $($s.TimeStamperCertificate.Subject)"
    Copy-Item $path (Join-Path $To $f) -Force
}
