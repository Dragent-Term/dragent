# Package Dragent for Windows (portable folder layout; basis for .msi).
#
# Layout (Release; Dev uses dist\windows-dragent-dev\ instead):
#   dist\windows-dragent[ -dev]\
#     dragent.exe
#     dragent-host.exe
#
# Examples:
#   .\scripts\package-windows.ps1
#
# Requires: PowerShell, prior `cargo build --release` (or use -Build)

param(
    [string]$Profile = "release",
    # Target architecture: x64 (default) or arm64. arm64 cross-compiles with
    # the aarch64-pc-windows-msvc target (VS Build Tools ARM64 libs required).
    [ValidateSet("x64", "arm64")]
    [string]$Arch = "x64",
    [switch]$Build,
    # Skip building the Inno Setup installer (still produces the portable zip).
    [switch]$NoInstaller
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$BinaryName = "dragent.exe"
$RustTarget = if ($Arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
# Prefer the explicit-target build dir; fall back to the host-default dir for
# local x64 builds done with plain `cargo build --release`.
$Built = Join-Path $Root "target\$RustTarget\$Profile\dragent.exe"
if (($Arch -eq "x64") -and -not (Test-Path $Built)) {
    $Built = Join-Path $Root "target\$Profile\dragent.exe"
}
# Edition split: the release workflow sets DRAGENT_RELEASE_EDITION=1 and gets
# the long-standing Release layout (AppName "Dragent", AppId
# {B7E4C9A2-...}, URL protocol "dragent"). Every other build is the Dev
# edition ("Dragent Dev", its own AppId, URL protocol "dragent-dev"), so it
# installs beside the Release app instead of upgrading it. Cargo inherits the
# same environment, so the binary's edition (see dragent_paths) always matches
# the installer it ships in.
$IsRelease = $env:DRAGENT_RELEASE_EDITION -eq "1"
$AppName = if ($IsRelease) { "Dragent" } else { "Dragent Dev" }
$StageDir = if ($IsRelease) { "windows-dragent" } else { "windows-dragent-dev" }
$Stage = Join-Path $Root "dist\$StageDir"
$Version = (Select-String -Path (Join-Path $Root "app\Cargo.toml") -Pattern '^version = ' | Select-Object -First 1).Line -replace '^version = "([^"]+)".*','$1'

if ($Build) {
    Write-Host "Building dragent ($Profile, $RustTarget) ..."
    Push-Location $Root
    rustup target add $RustTarget
    cargo build -p dragent -p dragent_host --profile $Profile --target $RustTarget
    Pop-Location
    $Built = Join-Path $Root "target\$RustTarget\$Profile\dragent.exe"
}

if (-not (Test-Path $Built)) {
    throw "Binary not found: $Built (run with -Build or cargo build first)"
}

# The agent launch monitor must be a console executable on Windows.
$HostBuilt = Join-Path (Split-Path $Built) "dragent-host.exe"
if (-not (Test-Path $HostBuilt)) {
    throw "Agent session helper not found: $HostBuilt (run with -Build or build -p dragent_host)"
}

if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Path $Stage | Out-Null
Copy-Item $Built (Join-Path $Stage $BinaryName)
Copy-Item $HostBuilt (Join-Path $Stage "dragent-host.exe")

# Runtime DLLs shipped beside the exe (DirectX/Vulkan etc.), if any.
Get-ChildItem (Join-Path (Split-Path $Built) "*.dll") -ErrorAction SilentlyContinue |
    ForEach-Object { Copy-Item $_.FullName $Stage }

# Ship the repo's skills/ so the agent has its built-in skills out of the box
# (skill discovery reads <install dir>\skills as the lowest-priority source).
$SkillsSrc = Join-Path $Root "skills"
if (Test-Path $SkillsSrc) {
    Copy-Item -Recurse $SkillsSrc (Join-Path $Stage "skills")
    $SkillCount = (Get-ChildItem -Recurse (Join-Path $Stage "skills") -Filter "SKILL.md").Count
    Write-Host "Bundled $SkillCount built-in skills"
} else {
    Write-Warning "No skills\ directory found - skipping bundled skills."
}

# Ship the third-party license notices with the app so every recipient receives
# a copy of each bundled open-source license. Required by the AGPL-3.0-only
# (sections 4-6) and by the MIT / SIL OFL / ISC / NCSA notice requirements.
foreach ($Legal in @("THIRD_PARTY_NOTICES.md", "NOTICE", "LICENSE")) {
    $LegalSrc = Join-Path $Root $Legal
    if (Test-Path $LegalSrc) { Copy-Item $LegalSrc (Join-Path $Stage $Legal) }
}
Write-Host "Bundled legal notices (THIRD_PARTY_NOTICES.md, NOTICE, LICENSE)"

$DistDir = Join-Path $Root "dist"
$Zip = Join-Path $DistDir "$AppName-$Version-windows-$Arch.zip"
if (Test-Path $Zip) { Remove-Item $Zip }
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Zip

# ── Inno Setup installer (.exe) ──────────────────────────────────────────────
# Compile scripts\windows\windows-installer.iss against the staged folder. The
# .iss reads /D-defined paths, so we hand it the staging dir, output dir, and the
# version parsed from Cargo.toml. Skipped (with a hint) if Inno Setup isn't found.
$Setup = $null
if (-not $NoInstaller) {
    $Iss = Join-Path $Root "scripts\windows\windows-installer.iss"
    $Iscc = (Get-Command "ISCC.exe" -ErrorAction SilentlyContinue).Source
    if (-not $Iscc) {
        foreach ($p in @(
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
            "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
        )) { if ($p -and (Test-Path $p)) { $Iscc = $p; break } }
    }
    if ($Iscc) {
        Write-Host "Building installer with Inno Setup ..."
        if ($IsRelease) {
            & $Iscc "/DAppVersion=$Version" "/DReleaseDir=$Stage" "/DOutDir=$DistDir" "/DArch=$Arch" $Iss
        } else {
            # Dev GUID generated once for this project (see windows-installer.iss):
            # it must stay constant across Dev installs so they upgrade each other.
            & $Iscc "/DAppVersion=$Version" "/DReleaseDir=$Stage" "/DOutDir=$DistDir" "/DArch=$Arch" '/DAppName=Dragent Dev' '/DAppId={{E0000677-10F0-44DA-8BAD-6CE519085F47}}' '/DUrlScheme=dragent-dev' '/DAppUserModelId=dev.dragent.app.dev' $Iss
        }
        if ($LASTEXITCODE -ne 0) { throw "ISCC failed (exit $LASTEXITCODE)" }
        $Setup = Join-Path $DistDir "$AppName-$Version-windows-$Arch-setup.exe"
    } else {
        Write-Warning "Inno Setup (ISCC.exe) not found - skipping installer. Install from https://jrsoftware.org/isdl.php (or pass -NoInstaller to silence this)."
    }
}

Write-Host ""
Write-Host "Windows packaging complete."
Write-Host "  Staging:   $Stage"
Write-Host "  Zip:       $Zip"
if ($Setup) { Write-Host "  Installer: $Setup" }
