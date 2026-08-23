# Generate winget manifests for a published release.
#
#   ./packaging/winget/make-manifests.ps1 -Version 0.1.0
#   ./packaging/winget/make-manifests.ps1 -Version 0.1.0 -Sha256 <hash>   # skip the download
#
# Writes manifests/<owner>.<name>.*.yaml with the version, download URL and
# SHA256 (fetched from the release's checksums.txt unless given).
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [string] $Sha256,
    [string] $OutDir = (Join-Path $PSScriptRoot 'manifests')
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..' 'product.ps1')
$Version = $Version.TrimStart('v')
$id = $WingetId
$url = "https://github.com/$ProductRepo/releases/download/v$Version/$ProductExe"

if (-not $Sha256) {
    $sumsUrl = "https://github.com/$ProductRepo/releases/download/v$Version/checksums.txt"
    Write-Host "Fetching $sumsUrl"
    $text = (Invoke-WebRequest $sumsUrl -UseBasicParsing).Content
    if ($text -is [byte[]]) { $text = [Text.Encoding]::ASCII.GetString($text) }  # pwsh 7 returns bytes for text/plain
    $line = ($text -split "`n" | Where-Object { $_ -match [regex]::Escape($ProductExe) } | Select-Object -First 1)
    if (-not $line) { throw "checksums.txt has no $ProductExe entry" }
    $Sha256 = ($line -split '\s+')[0]
}
$Sha256 = $Sha256.ToUpper()
if ($Sha256.Length -ne 64) { throw "SHA256 must be 64 hex chars, got '$Sha256'" }

New-Item -ItemType Directory -Force $OutDir | Out-Null
$tags = ($ProductTags | Sort-Object | ForEach-Object { "  - $_" }) -join "`n"

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.1.6.0.schema.json
PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $OutDir "$id.yaml")

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.1.6.0.schema.json
PackageIdentifier: $id
PackageVersion: $Version
InstallerType: portable
Commands:
  - $ProductName
ReleaseDate: $(Get-Date -Format 'yyyy-MM-dd')
Installers:
  - Architecture: x64
    InstallerUrl: $url
    InstallerSha256: $Sha256
ManifestType: installer
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $OutDir "$id.installer.yaml")

@"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.1.6.0.schema.json
PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: en-US
Publisher: $ProductOwner
PublisherUrl: https://github.com/$ProductOwner
PublisherSupportUrl: https://github.com/$ProductRepo/issues
PackageName: $ProductName
PackageUrl: https://github.com/$ProductRepo
License: MIT
LicenseUrl: https://github.com/$ProductRepo/blob/main/LICENSE
ShortDescription: $ProductDesc
Moniker: $ProductName
Tags:
$tags
ReleaseNotesUrl: https://github.com/$ProductRepo/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: 1.6.0
"@ | Set-Content -Encoding utf8 (Join-Path $OutDir "$id.locale.en-US.yaml")

Write-Host "Wrote manifests for $id $Version (sha256 $Sha256) to $OutDir"
Write-Host "Validate with:  winget validate --manifest $OutDir"
