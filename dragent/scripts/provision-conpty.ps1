<#
.SYNOPSIS
Provision a pinned, modern ConPTY (conpty.dll + OpenConsole.exe) next to dragent.exe.

.DESCRIPTION
dragent's PTY layer (portable-pty) prefers a conpty.dll sideloaded beside the
executable over the in-box Windows conhost, which varies by OS build and has
known escape-sequence ordering quirks (the reason for the drain/fence
architecture in terminal_session_core). Bundling pins the exact console
behavior we test against, on every customer machine.

Source: Microsoft's official ConPTY NuGet package, published with Windows
Terminal releases (MIT). Pinned by version AND SHA256 — a changed hash fails
the provision rather than silently shipping different bytes.

Windows-only by nature: Unix PTYs are kernel primitives and need no bundling.

.PARAMETER DestDir
Directory to place conpty.dll + OpenConsole.exe (default: target\debug next to
this repo's build output). Pass your packaging/dist directory when building
installers.

.PARAMETER Arch
Target architecture: arm64, x64, or x86. Defaults to this machine's.
#>
param(
    [string]$DestDir = (Join-Path $PSScriptRoot "..\target\debug"),
    [ValidateSet("arm64", "x64", "x86")]
    [string]$Arch = $(switch ($env:PROCESSOR_ARCHITECTURE) {
        "ARM64" { "arm64" }
        "AMD64" { "x64" }
        default { "x86" }
    })
)

$ErrorActionPreference = "Stop"

# Pinned release — bump BOTH together after verifying with the ps_repro harness.
$Version = "1.24.260512001"
$Url = "https://github.com/microsoft/terminal/releases/download/v1.24.11321.0/Microsoft.Windows.Console.ConPTY.$Version.nupkg"
$Sha256 = "3C66A99D38B5C2AC4C7552B7632CBBEF23A1911ACA5E20370109EB555A15D077"

$stamp = Join-Path $DestDir "conpty.version"
if ((Test-Path $stamp) -and (Get-Content $stamp -ErrorAction SilentlyContinue) -eq "$Version-$Arch" `
    -and (Test-Path (Join-Path $DestDir "conpty.dll")) `
    -and (Test-Path (Join-Path $DestDir "OpenConsole.exe"))) {
    Write-Host "ConPTY $Version ($Arch) already provisioned in $DestDir"
    exit 0
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "dragent-conpty-$Version.nupkg"
Write-Host "Downloading ConPTY $Version..."
Invoke-WebRequest -Uri $Url -OutFile $tmp

$actual = (Get-FileHash $tmp -Algorithm SHA256).Hash
if ($actual -ne $Sha256) {
    Remove-Item $tmp -Force
    throw "SHA256 mismatch for $Url`nexpected: $Sha256`nactual:   $actual"
}

New-Item -ItemType Directory -Force $DestDir | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($tmp)
try {
    $wanted = @{
        "runtimes/win-$Arch/native/conpty.dll"          = "conpty.dll"
        "build/native/runtimes/$Arch/OpenConsole.exe"   = "OpenConsole.exe"
    }
    foreach ($entryPath in $wanted.Keys) {
        $entry = $zip.Entries | Where-Object { $_.FullName -eq $entryPath }
        if (-not $entry) { throw "package layout changed: $entryPath not found" }
        $dest = Join-Path $DestDir $wanted[$entryPath]
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $dest, $true)
        Write-Host "  -> $dest"
    }
} finally {
    $zip.Dispose()
    Remove-Item $tmp -Force
}

Set-Content -Path $stamp -Value "$Version-$Arch" -Encoding ascii
Write-Host "ConPTY $Version ($Arch) provisioned. dragent will log 'bundled ConPTY' at startup."
