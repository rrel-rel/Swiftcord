<#
.SYNOPSIS
    Builds Swiftcord installers and update packages with Velopack, optionally publishing them to GitHub Releases.

.EXAMPLE
    ./build/release.ps1 -Version 0.2.0
    ./build/release.ps1 -Version 0.2.0 -Upload    # needs $env:GITHUB_TOKEN with contents:write

.NOTES
    Each architecture is its own Velopack channel (win-x64, win-arm64), so installed copies only ever
    receive updates for their own architecture. Output goes to artifacts/releases.
#>
param(
    [Parameter(Mandatory)][string]$Version,
    [ValidateSet('x64', 'arm64')][string[]]$Architectures = @('x64', 'arm64'),
    [switch]$Upload
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$repoUrl = 'https://github.com/rrel-rel/Swiftcord'
$vpkVersion = '1.2.161' # Keep in step with the Velopack package in Swiftcord.App.csproj.

dotnet tool update --global vpk --version $vpkVersion
if ($LASTEXITCODE) { throw 'Installing vpk failed.' }

foreach ($arch in $Architectures) {
    $platform = if ($arch -eq 'arm64') { 'ARM64' } else { 'x64' }
    $publishDir = Join-Path $root "artifacts/publish/$arch"
    $releaseDir = Join-Path $root 'artifacts/releases'
    $channel = "win-$arch"

    Remove-Item $publishDir -Recurse -Force -ErrorAction SilentlyContinue
    dotnet publish (Join-Path $root 'src/Swiftcord.App') -c Release -r "win-$arch" -p:Platform=$platform `
        -p:Version=$Version --self-contained -o $publishDir
    if ($LASTEXITCODE) { throw "Publishing $arch failed." }

    if ($Upload) {
        # Download the previous release so vpk can build a delta package against it.
        vpk download github --repoUrl $repoUrl --channel $channel -o $releaseDir
    }

    vpk pack --packId Swiftcord.Windows --packVersion $Version --packTitle Swiftcord --packAuthors Swiftcord `
        --packDir $publishDir --mainExe Swiftcord.exe --channel $channel -o $releaseDir
    if ($LASTEXITCODE) { throw "Packing $arch failed." }

    if ($Upload) {
        if (-not $env:GITHUB_TOKEN) { throw 'Set GITHUB_TOKEN to upload.' }
        vpk upload github --repoUrl $repoUrl --token $env:GITHUB_TOKEN --channel $channel -o $releaseDir `
            --publish --merge --releaseName "Swiftcord $Version (Windows)" --tag "windows-v$Version"
        if ($LASTEXITCODE) { throw "Uploading $arch failed." }
    }
}

Write-Host "Done. Installers: $(Join-Path $root 'artifacts/releases')"
