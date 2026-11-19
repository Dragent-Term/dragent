# Desktop computer use

This crate implements desktop access through macOS Accessibility and Quartz,
Windows UI Automation and SendInput, and Linux AT-SPI with desktop input tools.
It is a standalone Dragent subsystem with no dependency on the app UI.

The app owns authorization. CLI `dragent computer`, MCP `computer_*` tools, and
the built-in agent's `computer` tool all use the same app handler and settings.
The handler checks the master switch, resolves and checks the target app name
or id, and requests approval for every action that changes desktop state.
Permission and capability diagnostics remain available while computer use is disabled.
Approvals expire after 110 seconds. Remembered app approvals last for the
lifetime of the workspace window. Settings and identity are checked again
on the worker after any queue wait, before an approved request runs.

The backend serializes access on a worker thread. `preview_for` resolves the
actual target process; `run_for` binds the approved request to that process.
Snapshots are scoped to the caller and organisation, expire after two minutes,
and become invalid after an action on their app. The acting caller receives a fresh
snapshot with new, usable elements. A changed snapshot cannot
silently change an action that is waiting for approval. Requests that expire
in the worker queue are discarded. A running request that times out may have
partially completed; inspect the app before retrying.

Password managers and the current process are excluded. Password field values
are hidden in the accessibility tree. Screenshots are actual images and can
include sensitive visual content. Captures use a private temporary directory
and retain at most 30 prior images. Successful input dispatch is not proof
that an app completed its intended task. Each action carries verification metadata
and refreshed state; readback proves a direct field change, while synthetic
input and unasserted accessibility actions stay unverified.

## Platform requirements

- macOS: grant Accessibility and Screen Recording to the built Dragent app in
  System Settings. Settings → Computer use shows their status and opens the
  relevant pages. Text snapshots work without screen capture permission.
- Windows: UI Automation is built in. Higher integrity/elevated windows may be
  inaccessible to a normal process. The desktop worker uses physical pixel
  coordinates with per-monitor DPI awareness.
- Linux: a desktop session with AT-SPI enabled and `at-spi2-core` installed.
  X11 keyboard/scroll input requires `xdotool`; accessible element actions and
  text values do not. X11 screenshots identify the target client by PID and
  title instead of capturing a desktop crop. Wayland input requires `ydotool`
  and its daemon; screenshots require `grim` and compositor support. On
  Wayland the target must be in front and unobscured for its visible-region
  screenshot. Some compositors do not allow programmatic activation; the
  person must bring the target forward. Unsupported capabilities return
  explicit errors; text/accessibility operations remain available.

## Commands and validation

Run `dragent computer help` for command arguments. Window numbers start at 1;
accessibility element numbers start at 0. Click coordinates are relative to
the window in points (physical pixels on Windows/Linux). Divide screenshot
pixel coordinates by its reported scale first. Use `--text-stdin` with `type` or
`paste`, and `--value-stdin` with `set-value`, to keep text out of shell history.

The CLI, MCP and native tool support drag, clipboard paste, modifier-click,
advertised secondary actions, hotkeys, explicit stable window ids or one-based
indexes, and optional restore/capture settings on each action. Keyboard input
requires the selected window in front; typing and paste also require a focused
text receiver. Clipboard paste preserves existing text/HTML where possible;
other clipboard formats can be replaced.

Settings provides permission reset with explicit confirmation, live status,
and installation of the dedicated desktop-control skill. User-edited skill
files are preserved. `dragent computer guide` returns the bundled guide.

MCP observations and actions return text and image content. Native results become a
user image attachment after their tool result, using the same image format
as cloud and direct-provider user attachments. Images larger than 5 MiB fall
back to the text tree. Models must support images to inspect screenshots.

```sh
cargo test -p dragent_computer -p dragent_agent -p dragent_host --lib
cargo test -p dragent --lib
cargo check -p dragent_computer --all-targets --target x86_64-pc-windows-msvc
cargo check -p dragent_computer --all-targets --target x86_64-unknown-linux-gnu
```

The `probe` example exercises the backend directly for development; it does
not apply app settings or show app approvals. Use an isolated process with
its own OS permissions. Platform compilation and unit tests do not replace
live input and capture tests on each operating system.
