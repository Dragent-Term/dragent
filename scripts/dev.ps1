# Run Dragent Dev: build and launch the desktop app as the Dev edition.
#
# The Dev edition is every build made WITHOUT DRAGENT_RELEASE_EDITION=1 in the
# environment (debug or --release, ours or a contributor's). It keeps its own
# data folder (~/.dragent-dev, or DRAGENT_DEV_HOME), its own background
# services, its own Keychain entries and its own dragent-dev:// link scheme, so
# running it never touches the installed Dragent. On first launch it copies
# preference files once from the release folder, then never again.
#
# Usage:
#   scripts/dev.ps1 [-Account | -Server <url>] [-Fresh] [-ReleaseProfile] [-Help] [-- app-args...]
#
# Double-dash spellings (--account, --server, --fresh, --release-profile, --help) work too.
#
# Examples:
#   scripts/dev.ps1                           # local mode: no server, no account needed
#   scripts/dev.ps1 -Account                  # sign in to your own Dragent account
#   scripts/dev.ps1 -Server http://localhost:8080   # a Dragent server you run yourself
#   scripts/dev.ps1 -Fresh                    # confirm, delete the Dev data folder, start clean
#   scripts/dev.ps1 -ReleaseProfile           # build with --release (still the Dev edition)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = Split-Path -Parent $ScriptDir
$DragentDir = Join-Path $Root "dragent"

function Show-Usage {
  @'
Usage: scripts/dev.ps1 [options] [-- app-args...]

Build and run the desktop app as the Dev edition. The Dev edition never
touches the installed Dragent: its own data folder, background services,
Keychain entries and dragent-dev:// links.

Options:
  -Account                 Sign in to your own Dragent account, on the same
                           service as your installed Dragent (dev.ps1 asks the
                           installed app for its address), as a separate
                           device: your installed Dragent stays signed in and
                           untouched. Needs Dragent installed; without it,
                           this runs in local mode.
  -Server <url>            Use a Dragent server you run yourself, by its
                           http(s):// backend URL.
                           Default (neither): local mode, no server.
  -Fresh                   After a confirmation prompt, delete the Dev data
                           folder (~/.dragent-dev, or DRAGENT_DEV_HOME) so the
                           next launch starts clean (settings are copied once
                           again on launch).
  -ReleaseProfile          Build with --release (still the Dev edition).
  -Help                    Show this help.
  --                       End of dev.ps1 options; the rest is passed to the app.
                           (Options must come first: the first app argument
                           ends option parsing and is passed through with the
                           rest, so app flags work with or without `--`.)

Environment:
  DRAGENT_DEV_HOME         Override the Dev data folder (default ~/.dragent-dev).
  DRAGENT_DEV_SERVER       Set by -Account / -Server. Without either, dev.ps1
                           clears it so the build runs in local mode even if
                           set.
  DRAGENT_BACKEND_URL, DRAGENT_WEB_ORIGIN
                           Always cleared: a Dev build talks only to
                           DRAGENT_DEV_SERVER (or to no server in local mode).
                           With -Account, DRAGENT_WEB_ORIGIN is set to the
                           installed Dragent's web address.

Local mode (the default) needs no server and no account: the app opens
straight into the workspace as a local 'Dragent Dev' user. Features that need
an account show 'Needs a Dragent account' (plan and usage, members, teams,
billing, phone pairing and browser sign-in, organisation tasks, team
automations, server-held connections). The built-in agent still works with
your own API key. Use -Account to sign in and use account features.
'@
}

# Fatal error: message to stderr, exit 1. (Write-Error is avoided for control
# flow: under $ErrorActionPreference = "Stop" it would throw instead of
# reaching `exit`, leaving the exit code to chance.)
function Fail([string]$Message) {
  [Console]::Error.WriteLine($Message)
  exit 1
}

# Dev.ps1 options must come first. The first app argument ends option parsing:
# everything from there on (including anything looking like a flag) is passed
# to the app unchanged. `--` also ends option parsing explicitly, but note
# PowerShell itself consumes a bare `--` when the script is invoked with `&`
# from an interactive prompt, so app flags work with or without it.

$Server = ""
$Account = $false
$Fresh = $false
$ReleaseProfile = $false
$AppArgs = @()

$i = 0
$ParsingOptions = $true
while ($i -lt $args.Count) {
  $a = [string]$args[$i]
  if ($ParsingOptions -and ($a -eq "--")) {
    $ParsingOptions = $false
    $i++
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/)(h|help|\?)$')) {
    Show-Usage
    exit 0
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/)account$')) {
    $Account = $true
    $i++
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/)server$')) {
    if ($i + 1 -ge $args.Count) {
      Fail "scripts/dev.ps1: -Server requires the http(s):// URL of your own Dragent server."
    }
    $Server = [string]$args[$i + 1]
    $i += 2
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/)server=(.*)$')) {
    $Server = $Matches[2]
    $i++
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/)fresh$')) {
    $Fresh = $true
    $i++
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/)release-?profile$')) {
    $ReleaseProfile = $true
    $i++
  } elseif ($ParsingOptions -and ($a -match '^(account|server|fresh|release-?profile)$')) {
    # A forgotten "-" would otherwise start local mode without a word.
    Fail "scripts/dev.ps1: did you mean -${a}?"
  } elseif ($ParsingOptions -and ($a -match '^(-{1,2}|/).+')) {
    # Not a dev.ps1 flag: the app's own flags start here. (PowerShell consumes
    # a bare `--` separator for script invocations, so this branch is what
    # `dev.ps1 -Account -- --app-flag` reaches.)
    $ParsingOptions = $false
    $AppArgs += $a
    $i++
  } else {
    # A positional app argument: option parsing ends here too, so flags
    # after it belong to the app (`dev.ps1 . --app-flag` just works).
    $ParsingOptions = $false
    $AppArgs += $a
    $i++
  }
}

if ($Account -and -not [string]::IsNullOrEmpty($Server)) {
  Fail "scripts/dev.ps1: use either -Account or -Server <url>, not both."
}
if (-not [string]::IsNullOrEmpty($Server) -and ($Server -notmatch '^https?://.+')) {
  Fail "scripts/dev.ps1: -Server takes the http(s):// URL of a Dragent server you run yourself (got '$Server'); use -Account to sign in to your own Dragent account."
}

# The release workflow compiles with DRAGENT_RELEASE_EDITION=1, which makes a
# Release build that uses the installed app's data. Never build that here:
# refuse instead of silently producing it.
if (-not [string]::IsNullOrEmpty($env:DRAGENT_RELEASE_EDITION)) {
  Fail "scripts/dev.ps1: refusing to run with DRAGENT_RELEASE_EDITION set ($env:DRAGENT_RELEASE_EDITION). Unset it first: this script always builds the Dev edition."
}

# A dev build is usually started from a terminal of the installed app, so it
# inherits that app's daemon hand-off variables. Carried on, they would bind
# this build (and the services it starts) to the other app's daemons and
# terminal identity. Main strips them too; clearing them here keeps them out
# of the build itself. DRAGENT_PTY_LISTENER_SOCKET rides with the same
# hand-off group (see dragent_paths::PTY_HANDOFF_VARS).
@(
  "DRAGENT_PTY_LISTENER_FD",
  "DRAGENT_PTY_LISTENER_SOCKET",
  "DRAGENT_PTY_HANDOFF_SNAPSHOT",
  "DRAGENT_PTY_HANDOFF_FD",
  "DRAGENT_PTY_TOKEN",
  "DRAGENT_HOST_WORKSPACE_ID",
  "DRAGENT_HOST_TERMINAL_ID",
  # DRAGENT_HOME names the installed app's data folder. A dev build resolves
  # its own folder from DRAGENT_DEV_HOME (else ~/.dragent-dev) and must never
  # run on the installed app's data, so never inherit it.
  "DRAGENT_HOME",
  # A Dev build in local mode contacts no server, and with -Account or -Server
  # it talks only to DRAGENT_DEV_SERVER. Stale DRAGENT_BACKEND_URL / DRAGENT_WEB_ORIGIN
  # from the installed app (or the person's shell) must not leak in and confuse
  # this build or anything it starts.
  "DRAGENT_BACKEND_URL",
  "DRAGENT_WEB_ORIGIN"
) | ForEach-Object { Remove-Item "Env:$_" -ErrorAction SilentlyContinue }

# -Account: the backend and web addresses the installed Dragent was built
# with, from its read-only `service-address` command. $null when there's no
# installed Dragent, or it's too old to have the command. A Dev build answers
# nothing (no addresses built in), so a Dev launcher on PATH is passed over.
function Read-InstalledService {
  $Candidates = @()
  foreach ($Base in @($env:LOCALAPPDATA, $env:ProgramFiles, ${env:ProgramFiles(x86)})) {
    if (-not [string]::IsNullOrEmpty($Base)) {
      $Candidates += ,@((Join-Path $Base "Programs\Dragent\dragent.exe"), $true)
      $Candidates += ,@((Join-Path $Base "Dragent\dragent.exe"), $true)
    }
  }
  foreach ($Path in @(
      "/Applications/Dragent.app/Contents/MacOS/dragent",
      (Join-Path ([string]$HOME) "Applications/Dragent.app/Contents/MacOS/dragent"),
      "/opt/Dragent/dragent",
      "/opt/dragent/dragent",
      "/usr/lib/dragent/dragent")) {
    $Candidates += ,@($Path, $true)
  }
  # A launcher on PATH adds --dragent-cli itself.
  $OnPath = Get-Command dragent -CommandType Application, ExternalScript -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($OnPath) { $Candidates += ,@($OnPath.Source, $false) }

  foreach ($Candidate in $Candidates) {
    $Exe = $Candidate[0]
    if (-not (Test-Path -LiteralPath $Exe -PathType Leaf)) { continue }
    $CliArgs = @()
    if ($Candidate[1]) { $CliArgs += "--dragent-cli" }
    $CliArgs += @("service-address", "--json")
    try {
      $Out = & $Exe @CliArgs 2>$null
      if ($LASTEXITCODE -ne 0) { continue }
      $Json = ($Out -join "`n") | ConvertFrom-Json -ErrorAction Stop
    } catch {
      continue
    }
    $Backend = [string]$Json.backend
    $Web = [string]$Json.web
    if (($Backend -match '^https?://.+') -and ($Web -match '^https?://.+')) {
      return [pscustomobject]@{ Backend = $Backend; Web = $Web }
    }
  }
  return $null
}

# Server choice. Default: local mode, so clear any inherited value: a typo
# must never send a Dev build to some other address (the app treats an
# unusable value as no server). -Account / -Server set DRAGENT_DEV_SERVER for
# this run only.
$ServerDesc = "local (no server)"
Remove-Item "Env:DRAGENT_DEV_SERVER" -ErrorAction SilentlyContinue
if ($Account) {
  # The service's address is not written in this repository: ask the
  # installed Dragent which service it was built for, and sign in there.
  $Installed = Read-InstalledService
  if ($Installed) {
    $env:DRAGENT_DEV_SERVER = $Installed.Backend
    $env:DRAGENT_WEB_ORIGIN = $Installed.Web
    $ServerDesc = "your Dragent account (the service your installed Dragent uses)"
  } else {
    [Console]::Error.WriteLine("scripts/dev.ps1: Install Dragent and sign in once to use -Account; running in local mode.")
  }
} elseif (-not [string]::IsNullOrEmpty($Server)) {
  $env:DRAGENT_DEV_SERVER = $Server
  $ServerDesc = "your own server $Server"
}

# The edition's data folder. Mirrors dragent_paths::home() for the Dev
# edition's default: DRAGENT_DEV_HOME, else ~/.dragent-dev.
if (-not [string]::IsNullOrEmpty($env:DRAGENT_DEV_HOME)) {
  $DataDir = $env:DRAGENT_DEV_HOME
} else {
  if ([string]::IsNullOrEmpty($HOME)) {
    Fail "scripts/dev.ps1: HOME is not set and DRAGENT_DEV_HOME is empty; cannot locate the Dev data folder."
  }
  $DataDir = Join-Path $HOME ".dragent-dev"
}

$IsWindowsPath = ([System.IO.Path]::DirectorySeparatorChar -eq '\')

# Canonical form of a path: absolute and resolved the way the kernel (and the
# app) resolves it, with every existing component (including symlinks and
# junctions, even dangling ones) resolved to its target. Trailing components
# that do not exist are kept as-is, so this also works for folders that were
# already deleted or are about to be created. Uses .NET file APIs only
# (no provider cmdlets).
#
# NOTE: `..` is resolved by popping the already-resolved physical path, never
# by collapsing the input textually: textual collapse disagrees with the
# kernel when an earlier component is a symlink, and the kernel's answer is
# the only one that matters for a delete guard.
function Resolve-CanonicalPath([string]$Path) {
  if ([System.IO.Path]::IsPathRooted($Path)) {
    $abs = $Path
  } else {
    $abs = [System.IO.Path]::Combine([System.IO.Directory]::GetCurrentDirectory(), $Path)
  }
  $root = [System.IO.Path]::GetPathRoot($abs)
  $rest = $abs.Substring($root.Length)
  $current = $root
  foreach ($part in ($rest -split '[\\/]+')) {
    if ([string]::IsNullOrEmpty($part) -or $part -eq ".") { continue }
    if ($part -eq "..") {
      if (-not (Test-PathEqual $current $root)) {
        $idx = $current.LastIndexOfAny([char[]]@('/', '\'))
        $parent = if ($idx -gt 0) { $current.Substring(0, $idx) } else { $root }
        if ($parent.Length -lt $root.Length) { $parent = $root }
        $current = $parent
      }
      continue
    }
    $candidate = [System.IO.Path]::Combine($current, $part)
    # LinkTarget is visible even for dangling links, where Exists is false.
    $info = $null
    if ([System.IO.Directory]::Exists($candidate)) {
      $info = [System.IO.DirectoryInfo]::new($candidate)
    } elseif ([System.IO.File]::Exists($candidate)) {
      $info = [System.IO.FileInfo]::new($candidate)
    } else {
      $probe = [System.IO.FileInfo]::new($candidate)
      if ($probe.LinkTarget) { $info = $probe }
    }
    if ($info -and $info.LinkTarget) {
      $resolved = $info.ResolveLinkTarget($true)
      if ($resolved) {
        $current = $resolved.FullName
      } else {
        $t = [string]@($info.LinkTarget)[0]
        if (-not [System.IO.Path]::IsPathRooted($t)) {
          $t = [System.IO.Path]::Combine($current, $t)
        }
        $current = [System.IO.Path]::GetFullPath($t)
      }
    } elseif ($info) {
      $current = $info.FullName
    } else {
      $current = $candidate
    }
  }
  return $current
}

# True when $Path is $Base or lives inside it. Both must already be canonical.
# Case-insensitive on Windows, case-sensitive elsewhere.
function Test-PathWithin([string]$Path, [string]$Base) {
  if (Test-PathEqual $Path $Base) { return $true }
  if (Test-PathEqual $Base ([System.IO.Path]::GetPathRoot($Base))) { return $true }
  $prefix = $Base + [string][System.IO.Path]::DirectorySeparatorChar
  $comparison = [System.StringComparison]::Ordinal
  if ($IsWindowsPath) { $comparison = [System.StringComparison]::OrdinalIgnoreCase }
  return $Path.StartsWith($prefix, $comparison)
}

function Test-PathEqual([string]$A, [string]$B) {
  $comparison = [System.StringComparison]::Ordinal
  if ($IsWindowsPath) { $comparison = [System.StringComparison]::OrdinalIgnoreCase }
  return [string]::Equals($A, $B, $comparison)
}

if ($Fresh) {
  if ([string]::IsNullOrEmpty($DataDir)) {
    Fail "scripts/dev.ps1: refusing -Fresh: the Dev data folder is empty."
  }
  # -Fresh deletes a whole folder tree: first prove it cannot be the installed
  # app's data (or anything containing it), resolving symlinks so a link
  # pointing into the release folder is caught too. These checks run before
  # any prompt.
  if ([string]::IsNullOrEmpty($HOME)) {
    Fail "scripts/dev.ps1: refusing -Fresh: HOME is not set, so the installed app's data folder cannot be located."
  }
  $canonData = Resolve-CanonicalPath $DataDir
  $canonHome = Resolve-CanonicalPath $HOME
  $releaseDir = Join-Path $canonHome ".dragent"
  if (Test-PathEqual $canonData $releaseDir) {
    # It IS the installed app's data folder.
    Fail "scripts/dev.ps1: refusing -Fresh: '$DataDir' is the installed app's data folder ($releaseDir). Set DRAGENT_DEV_HOME to a different folder."
  }
  if (Test-PathEqual $canonData $canonHome) {
    Fail "scripts/dev.ps1: refusing -Fresh: '$DataDir' is your home directory."
  }
  if (($canonData -eq "/") -or ($canonData -match '^[a-zA-Z]:\\$')) {
    Fail "scripts/dev.ps1: refusing -Fresh: '$DataDir' is a filesystem root."
  }
  if (Test-PathWithin $canonData $releaseDir) {
    Fail "scripts/dev.ps1: refusing -Fresh: '$DataDir' is inside the installed app's data folder ($releaseDir). Set DRAGENT_DEV_HOME to a different folder."
  }
  if (Test-PathWithin $releaseDir $canonData) {
    Fail "scripts/dev.ps1: refusing -Fresh: '$DataDir' contains the installed app's data folder ($releaseDir); deleting it would destroy the installed app. Set DRAGENT_DEV_HOME to a different folder."
  }
  $answer = Read-Host "Delete the Dev data folder at '$DataDir'? This cannot be undone. [y/N]"
  if ($answer -match '^(?i:y|yes)$') {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue -- $DataDir
    Write-Host "Deleted $DataDir. It will be recreated on launch (settings copy runs once again)."
  } else {
    Write-Host "Kept $DataDir."
    exit 0
  }
}

Write-Host "Dragent Dev: data dir='$DataDir' server='$ServerDesc'"

$cargoArgs = @("run", "-p", "dragent")
if ($ReleaseProfile) {
  $cargoArgs += "--release"
}

Push-Location $DragentDir
try {
  # What a release ships beside dragent.exe: dragent-host.exe (agents start
  # without their hooks otherwise) and the pinned ConPTY (terminals fall back
  # to the in-box conhost otherwise, whose repaints make output flicker).
  $hostArgs = @("build", "-p", "dragent_host", "--bin", "dragent-host")
  if ($ReleaseProfile) { $hostArgs += "--release" }
  & cargo @hostArgs
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  # The ConPTY must match the app's architecture, which is the build target's
  # (CARGO_BUILD_TARGET, else the pinned toolchain's host), not this shell's:
  # an x64 PowerShell on Windows on ARM still builds an arm64 app.
  $profileDir = if ($ReleaseProfile) { "release" } else { "debug" }
  $triple = $env:CARGO_BUILD_TARGET
  if ([string]::IsNullOrEmpty($triple)) {
    $triple = ((& rustc -vV) | Select-String '^host:\s*(\S+)').Matches[0].Groups[1].Value
    $targetDir = Join-Path $DragentDir "target\$profileDir"
  } else {
    $targetDir = Join-Path $DragentDir "target\$triple\$profileDir"
  }
  $conptyArch = switch -Wildcard ($triple) {
    "aarch64-*" { "arm64" }
    "x86_64-*" { "x64" }
    "i686-*" { "x86" }
    default { $null }
  }
  if (-not $conptyArch) {
    Write-Warning "No pinned ConPTY for build target '$triple'; terminals will use the in-box conhost."
  } else {
    try {
      & (Join-Path $DragentDir "scripts\provision-conpty.ps1") -DestDir $targetDir -Arch $conptyArch
    } catch {
      Write-Warning "Could not provision the pinned ConPTY ($($_.Exception.Message)); terminals will use the in-box conhost."
    }
  }
  if ($AppArgs.Count -gt 0) {
    & cargo @cargoArgs -- @AppArgs
  } else {
    & cargo @cargoArgs
  }
  exit $LASTEXITCODE
} finally {
  Pop-Location
}
