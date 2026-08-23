# Submit (or update) the package on winget via wingetcreate.
#
#   ./packaging/winget/submit.ps1 -Version 0.1.0 -Token <github PAT with public_repo scope>
#   ./packaging/winget/submit.ps1 -Version 0.1.0 -Token $env:WINGET_TOKEN -First   # very first submission
#
# `wingetcreate update` needs the package to already exist in microsoft/winget-pkgs;
# the first submission is auto-detected (or forced with -First) and opens a
# new-package PR, which Microsoft reviews manually.
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $Token,
    [switch] $First,
    [switch] $DryRun
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..' 'product.ps1')
$Version = $Version.TrimStart('v')
$id = $WingetId
$url = "https://github.com/$ProductRepo/releases/download/v$Version/$ProductExe"

if (-not (Get-Command wingetcreate -ErrorAction SilentlyContinue)) {
    Write-Host "Installing wingetcreate…"
    winget install --id Microsoft.WingetCreate --exact --accept-source-agreements --accept-package-agreements --silent | Out-Null
}

if (-not $First) {
    $letter = $ProductOwner.Substring(0, 1).ToLower()
    $probe = "https://api.github.com/repos/microsoft/winget-pkgs/contents/manifests/$letter/$ProductOwner/$ProductName"
    try {
        Invoke-RestMethod -Uri $probe -Headers @{ Authorization = "Bearer $Token"; 'User-Agent' = "$ProductName-release" } | Out-Null
    } catch {
        Write-Host "$id is not in winget-pkgs yet - doing the first submission."
        $First = $true
    }
}

if ($First) {
    & (Join-Path $PSScriptRoot 'make-manifests.ps1') -Version $Version
    $dir = Join-Path $PSScriptRoot 'manifests'
    if ($DryRun) { winget validate --manifest $dir; return }
    wingetcreate submit --token $Token --prtitle "New package: $id version $Version" $dir
} else {
    $args = @('update', $id, '--version', $Version, '--urls', $url, '--token', $Token)
    if (-not $DryRun) { $args += '--submit' }
    wingetcreate @args
}
