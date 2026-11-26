#!/usr/bin/env bash
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
#   scripts/dev.sh [--account | --server <url>] [--fresh] [--release-profile] [-- app-args...]
#
# Examples:
#   scripts/dev.sh                       # local mode: no server, no account needed
#   scripts/dev.sh --account             # sign in to your own Dragent account
#   scripts/dev.sh --server http://localhost:8080   # a Dragent server you run yourself
#   scripts/dev.sh --fresh               # confirm, delete the Dev data folder, start clean
#   scripts/dev.sh --release-profile     # build with --release (still the Dev edition)
#
# Windows: run scripts/dev.ps1 instead (same options as -Account, -Server, -Fresh, ...).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DRAGENT_DIR="$ROOT/dragent"

usage() {
  cat <<'EOF'
Usage: scripts/dev.sh [options] [-- app-args...]

Build and run the desktop app as the Dev edition. The Dev edition never
touches the installed Dragent: its own data folder, background services,
Keychain entries and dragent-dev:// links.

Options:
  --account                Sign in to your own Dragent account, on the same
                           service as your installed Dragent (dev.sh asks the
                           installed app for its address), as a separate
                           device: your installed Dragent stays signed in and
                           untouched. Needs Dragent installed; without it,
                           this runs in local mode.
  --server <url>           Use a Dragent server you run yourself, by its
                           http(s):// backend URL.
                           Default (neither): local mode, no server.
  --fresh                  After a confirmation prompt, delete the Dev data
                           folder (~/.dragent-dev, or DRAGENT_DEV_HOME) so the
                           next launch starts clean (settings are copied once
                           again on launch).
  --release-profile        Build with --release (still the Dev edition).
  -h, --help               Show this help.
  --                       End of dev.sh options; the rest is passed to the app.

Environment:
  DRAGENT_DEV_HOME         Override the Dev data folder (default ~/.dragent-dev).
  DRAGENT_DEV_SERVER       Set by --account / --server. Without either, dev.sh
                           clears it so the build runs in local mode even if
                           exported.
  DRAGENT_BACKEND_URL, DRAGENT_WEB_ORIGIN
                           Always cleared: a Dev build talks only to
                           DRAGENT_DEV_SERVER (or to no server in local mode).
                           With --account, DRAGENT_WEB_ORIGIN is set to the
                           installed Dragent's web address.

Local mode (the default) needs no server and no account: the app opens
straight into the workspace as a local 'Dragent Dev' user. Features that need
an account show 'Needs a Dragent account' (plan and usage, members, teams,
billing, phone pairing and browser sign-in, organisation tasks, team
automations, server-held connections). The built-in agent still works with
your own API key. Use --account to sign in and use account features.
EOF
}

SERVER=""
ACCOUNT=0
FRESH=0
RELEASE_PROFILE=0
APP_ARGS=()

while [ $# -gt 0 ]; do
  case "$1" in
    --account)
      ACCOUNT=1
      shift
      ;;
    --server)
      if [ $# -lt 2 ]; then
        echo "scripts/dev.sh: --server requires the http(s):// URL of your own Dragent server" >&2
        exit 1
      fi
      SERVER="$2"
      shift 2
      ;;
    --server=*)
      SERVER="${1#--server=}"
      shift
      ;;
    --fresh)
      FRESH=1
      shift
      ;;
    --release-profile)
      RELEASE_PROFILE=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      while [ $# -gt 0 ]; do
        APP_ARGS+=("$1")
        shift
      done
      break
      ;;
    -*)
      echo "scripts/dev.sh: unknown option: $1 (see --help; app options go after --)" >&2
      exit 1
      ;;
    account|server|fresh|release-profile)
      # A forgotten "--" would otherwise start local mode without a word.
      echo "scripts/dev.sh: did you mean --$1? (app arguments go after --)" >&2
      exit 1
      ;;
    *)
      APP_ARGS+=("$1")
      shift
      ;;
  esac
done

if [ "$ACCOUNT" -eq 1 ] && [ -n "$SERVER" ]; then
  echo "scripts/dev.sh: use either --account or --server <url>, not both" >&2
  exit 1
fi
case "$SERVER" in
  ""|http://?*|https://?*) ;;
  *)
    echo "scripts/dev.sh: --server takes the http(s):// URL of a Dragent server you run yourself (got '$SERVER'); use --account to sign in to your own Dragent account" >&2
    exit 1
    ;;
esac

# The release workflow compiles with DRAGENT_RELEASE_EDITION=1, which makes a
# Release build that uses the installed app's data. Never build that here:
# refuse instead of silently producing it.
if [ -n "${DRAGENT_RELEASE_EDITION:-}" ]; then
  echo "scripts/dev.sh: refusing to run with DRAGENT_RELEASE_EDITION set (${DRAGENT_RELEASE_EDITION})." >&2
  echo "Unset it first: this script always builds the Dev edition." >&2
  exit 1
fi

# A dev build is usually started from a terminal of the installed app, so it
# inherits that app's daemon hand-off variables. Carried on, they would bind
# this build (and the services it starts) to the other app's daemons and
# terminal identity. Main strips them too; clearing them here keeps them out
# of the build itself. DRAGENT_PTY_LISTENER_SOCKET rides with the same
# hand-off group (see dragent_paths::PTY_HANDOFF_VARS).
unset DRAGENT_PTY_LISTENER_FD || true
unset DRAGENT_PTY_LISTENER_SOCKET || true
unset DRAGENT_PTY_HANDOFF_SNAPSHOT || true
unset DRAGENT_PTY_HANDOFF_FD || true
unset DRAGENT_PTY_TOKEN || true
unset DRAGENT_HOST_WORKSPACE_ID || true
unset DRAGENT_HOST_TERMINAL_ID || true
# DRAGENT_HOME names the installed app's data folder. A dev build resolves its
# own folder from DRAGENT_DEV_HOME (else ~/.dragent-dev) and must never run on
# the installed app's data, so never inherit it.
unset DRAGENT_HOME || true
# A Dev build in local mode contacts no server, and with --account or --server
# it talks only to DRAGENT_DEV_SERVER. Stale DRAGENT_BACKEND_URL / DRAGENT_WEB_ORIGIN
# from the installed app (or the person's shell) must not leak in and confuse
# this build or anything it starts.
unset DRAGENT_BACKEND_URL || true
unset DRAGENT_WEB_ORIGIN || true

# --account: the backend and web addresses the installed Dragent was built
# with, from its read-only `service-address` command. Sets INSTALLED_BACKEND
# and INSTALLED_WEB; fails when there's no installed Dragent, or it's too old
# to have the command. A Dev build answers nothing (no addresses built in),
# so a Dev launcher on PATH is passed over.
read_installed_service() {
  local candidate out backend web
  local -a cmd
  for candidate in \
      "/Applications/Dragent.app/Contents/MacOS/dragent" \
      "$HOME/Applications/Dragent.app/Contents/MacOS/dragent" \
      "/opt/Dragent/dragent" \
      "/opt/dragent/dragent" \
      "/usr/lib/dragent/dragent" \
      "PATH"; do
    if [ "$candidate" = "PATH" ]; then
      candidate="$(command -v dragent 2>/dev/null || true)"
      [ -n "$candidate" ] || continue
      cmd=("$candidate")            # a launcher adds --dragent-cli itself
    else
      [ -x "$candidate" ] || continue
      cmd=("$candidate" --dragent-cli)
    fi
    out="$("${cmd[@]}" service-address --json 2>/dev/null </dev/null)" || continue
    backend="$(printf '%s' "$out" | sed -n 's/.*"backend"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    web="$(printf '%s' "$out" | sed -n 's/.*"web"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    case "$backend" in http://?*|https://?*) ;; *) continue ;; esac
    case "$web" in http://?*|https://?*) ;; *) continue ;; esac
    INSTALLED_BACKEND="$backend"
    INSTALLED_WEB="$web"
    return 0
  done
  return 1
}

# Server choice. Default: local mode, so clear any exported value: a typo must
# never send a Dev build to some other address (the app treats an unusable
# value as no server). --account / --server set DRAGENT_DEV_SERVER for this
# run only.
SERVER_DESC="local (no server)"
unset DRAGENT_DEV_SERVER || true
if [ "$ACCOUNT" -eq 1 ]; then
  # The service's address is not written in this repository: ask the
  # installed Dragent which service it was built for, and sign in there.
  if read_installed_service; then
    export DRAGENT_DEV_SERVER="$INSTALLED_BACKEND"
    export DRAGENT_WEB_ORIGIN="$INSTALLED_WEB"
    SERVER_DESC="your Dragent account (the service your installed Dragent uses)"
  else
    echo "scripts/dev.sh: Install Dragent and sign in once to use --account; running in local mode." >&2
  fi
elif [ -n "$SERVER" ]; then
  export DRAGENT_DEV_SERVER="$SERVER"
  SERVER_DESC="your own server $SERVER"
fi

# The edition's data folder. Mirrors dragent_paths::home() for the Dev
# edition's default: DRAGENT_DEV_HOME, else ~/.dragent-dev.
if [ -n "${DRAGENT_DEV_HOME:-}" ]; then
  DATA_DIR="${DRAGENT_DEV_HOME}"
else
  if [ -z "${HOME:-}" ]; then
    echo "scripts/dev.sh: HOME is not set and DRAGENT_DEV_HOME is empty; cannot locate the Dev data folder." >&2
    exit 1
  fi
  DATA_DIR="$HOME/.dragent-dev"
fi

# Print the canonical form of a path: absolute and resolved the way the
# kernel (and the app) resolves it, with every existing component (including
# symlinks) made physical. Trailing components that do not exist are kept
# as-is, so this also works for folders that were already deleted or are
# about to be created. Returns nonzero when nothing on the way exists.
canonical_path() {
  local path="$1"
  case "$path" in
    /*) ;;
    *) path="$PWD/$path" ;;
  esac
  # Peel off trailing components that do not exist, then resolve the rest.
  local leaf=""
  local parent
  while [ ! -e "$path" ] && [ ! -L "$path" ]; do
    leaf="/$(basename "$path")$leaf"
    parent="$(dirname "$path")"
    if [ "$parent" = "$path" ]; then
      break
    fi
    path="$parent"
  done
  # NOTE: plain `cd` resolves `..` lexically, which disagrees with the kernel
  # (and the app) when an earlier component is a symlink (`-e` above follows
  # the kernel). Always use physical mode (`-P`) so the result is where the
  # app would really read and write.
  if [ -d "$path" ]; then
    path="$(cd -P "$path" && pwd -P)" || return 1
  elif [ -e "$path" ] || [ -L "$path" ]; then
    local dir base target
    dir="$(cd -P "$(dirname "$path")" && pwd -P)" || return 1
    base="$(basename "$path")"
    path="$dir/$base"
    # Resolve a file-level symlink one hop at a time.
    while [ -L "$path" ]; do
      target="$(readlink "$path")" || return 1
      case "$target" in
        /*) path="$target" ;;
        *) path="$(dirname "$path")/$target" ;;
      esac
      dir="$(cd -P "$(dirname "$path")" 2>/dev/null && pwd -P)" || return 1
      path="$dir/$(basename "$path")"
    done
  else
    return 1
  fi
  path="$path$leaf"
  # Collapse any doubled slashes (root + leading slash) for stable compares.
  case "$path" in
    *"//"*) path="$(printf '%s\n' "$path" | tr -s '/')" ;;
  esac
  printf '%s\n' "$path"
}

# 0 when $1 is $2 or lives inside it. Both must already be canonical, with no
# trailing slash (except that $2 may be /).
path_within() {
  local path="$1" base="$2"
  if [ "$path" = "$base" ]; then
    return 0
  fi
  if [ "$base" = "/" ]; then
    return 0
  fi
  case "$path" in
    "$base"/*) return 0 ;;
  esac
  return 1
}

if [ "$FRESH" = 1 ]; then
  if [ -z "${DATA_DIR:-}" ]; then
    echo "scripts/dev.sh: refusing --fresh: the Dev data folder is empty." >&2
    exit 1
  fi
  # --fresh deletes a whole folder tree: first prove it cannot be the installed
  # app's data (or anything containing it), resolving symlinks so a link
  # pointing into ~/.dragent is caught too. These checks run before any prompt.
  if [ -z "${HOME:-}" ]; then
    echo "scripts/dev.sh: refusing --fresh: HOME is not set, so the installed app's data folder cannot be located." >&2
    exit 1
  fi
  CANON_DATA=""
  CANON_HOME=""
  if ! CANON_DATA="$(canonical_path "$DATA_DIR")" || [ -z "$CANON_DATA" ]; then
    echo "scripts/dev.sh: refusing --fresh: cannot resolve '$DATA_DIR'." >&2
    exit 1
  fi
  if ! CANON_HOME="$(canonical_path "$HOME")" || [ -z "$CANON_HOME" ]; then
    echo "scripts/dev.sh: refusing --fresh: cannot resolve '$HOME'." >&2
    exit 1
  fi
  RELEASE_DIR="$CANON_HOME/.dragent"
  if [ "$CANON_DATA" = "$RELEASE_DIR" ]; then
    echo "scripts/dev.sh: refusing --fresh: '$DATA_DIR' is the installed app's data folder ($RELEASE_DIR). Set DRAGENT_DEV_HOME to a different folder." >&2
    exit 1
  fi
  if [ "$CANON_DATA" = "$CANON_HOME" ]; then
    echo "scripts/dev.sh: refusing --fresh: '$DATA_DIR' is your home directory." >&2
    exit 1
  fi
  if [ "$CANON_DATA" = "/" ]; then
    echo "scripts/dev.sh: refusing --fresh: '$DATA_DIR' is the filesystem root." >&2
    exit 1
  fi
  if path_within "$CANON_DATA" "$RELEASE_DIR"; then
    echo "scripts/dev.sh: refusing --fresh: '$DATA_DIR' is inside the installed app's data folder ($RELEASE_DIR). Set DRAGENT_DEV_HOME to a different folder." >&2
    exit 1
  fi
  if path_within "$RELEASE_DIR" "$CANON_DATA"; then
    echo "scripts/dev.sh: refusing --fresh: '$DATA_DIR' contains the installed app's data folder ($RELEASE_DIR); deleting it would destroy the installed app. Set DRAGENT_DEV_HOME to a different folder." >&2
    exit 1
  fi
  printf "Delete the Dev data folder at '%s'? This cannot be undone. [y/N] " "$DATA_DIR"
  ANSWER=""
  read -r ANSWER || true
  case "$ANSWER" in
    y|Y|yes|YES)
      rm -rf -- "$DATA_DIR"
      echo "Deleted $DATA_DIR. It will be recreated on launch (settings copy runs once again)."
      ;;
    *)
      echo "Kept $DATA_DIR."
      exit 0
      ;;
  esac
fi

printf "Dragent Dev: data dir='%s' server='%s'\n" "$DATA_DIR" "$SERVER_DESC"

CARGO_ARGS="run -p dragent"
if [ "$RELEASE_PROFILE" = 1 ]; then
  CARGO_ARGS="$CARGO_ARGS --release"
fi

cd "$DRAGENT_DIR"
# shellcheck disable=SC2086
if [ "${#APP_ARGS[@]}" -gt 0 ]; then
  exec cargo $CARGO_ARGS -- "${APP_ARGS[@]}"
else
  # shellcheck disable=SC2086
  exec cargo $CARGO_ARGS
fi
