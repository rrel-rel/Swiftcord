# Builds the release exes (x64 and ARM64) and the installer, target\installer\08cord-Setup-<version>.exe.
# Needs the x86_64-pc-windows-msvc and aarch64-pc-windows-msvc Rust targets, the MSVC build tools for both, and
# Inno Setup 6 (winget install JRSoftware.InnoSetup).
param([switch]$SkipBuild)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\.."
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

if (-not $SkipBuild) {
    # Build inside the MSVC environment for each target, so the linker finds that architecture's libraries
    # (an install without them, like VS Community's ARM-only one, would otherwise be picked).
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $vs = & $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -latest -property installationPath
    if (-not $vs) { throw "no Visual Studio install has the x64 C++ build tools" }
    $vcvars = "$vs\VC\Auxiliary\Build\vcvarsall.bat"
    $hostArch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "arm64" } else { "amd64" }
    foreach ($t in @(@{ target = "x86_64-pc-windows-msvc"; arch = "amd64" }, @{ target = "aarch64-pc-windows-msvc"; arch = "arm64" })) {
        $vcArch = if ($hostArch -eq $t.arch) { $t.arch } else { "${hostArch}_$($t.arch)" }
        Write-Host "Building $($t.target)…"
        cmd /c "call `"$vcvars`" $vcArch >nul 2>&1 && cargo build --release --target $($t.target) -p swiftcord --manifest-path `"$root\Cargo.toml`""
        if ($LASTEXITCODE -ne 0) { throw "build for $($t.target) failed" }
    }
}

$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 isn't installed (winget install JRSoftware.InnoSetup)" }
& $iscc "/DAppVersion=$version" "$root\installer\swiftcord.iss"
if ($LASTEXITCODE -ne 0) { throw "the installer didn't build" }
Get-Item "$root\target\installer\08cord-Setup-$version.exe"
