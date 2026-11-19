# Native terminal viewer

`createTerminalState()` creates an empty model. Feed every gateway delta to
`reduceTerminalDelta(state, delta)` from the synchronous delta subscription, then
render `TerminalView` with the resulting state and optional `content`:
`'all'` (default) interleaves commands and agent conversation, `'terminal'`
shows command blocks and full-screen snapshots only, and `'agent'` shows only
the agent conversation. The reducer has no React or native
runtime dependencies. The renderer requires the existing theme provider and a
bounded-height parent; it owns its vertical FlatList, so do not nest it inside a
vertical ScrollView. Routes, subscriptions and input controls remain outside this
module.

Command starts, styled row patches and completion events upsert by block id.
`base_line` replaces only the announced absolute row range; missing rows do not
allocate blank arrays and shorter patches do not delete the remaining tail.
Repeated starts preserve completed status during attach replay. Surface state and
agent block lists replace their previous snapshots. Agent payloads become bounded,
read-only text: approvals, questions, tools, URLs and file data never become local
actions or embedded HTML.

Storage retains at most 128 command blocks, 500 rows per block, 2,000 command rows
overall and 512,000 command-output characters. Individual rows allow 8,192
characters and 128 spans. Agent snapshots retain 128 top-level entries of at most
8,192 characters each, with bounded nested traversal. Alternate-screen snapshots
retain 240 rows with the same per-row bounds. Metadata and surface fields are also
bounded. Removed history is marked in the UI; replay cannot restore rows below a
retained block's discard boundary. Root should create a fresh state when changing
sessions.

The selected terminal theme supplies default foreground/background colors.
Explicit host-resolved RGBA span colors take precedence. The renderer supports
bold, dim, italic, inverse, concealment, underline and strikethrough; double/wavy
underlines use a single underline, and blink/overline have no native equivalent.
Rows use selectable native monospace text and horizontal scrolling without
wrapping. Command and agent blocks share one virtualized list and follow
lifecycle, interleaved by host
timestamps. Output follows only when the reader was at the bottom; native visible
content anchoring keeps earlier reading positions when retained history changes.
Agent rows use typography and semantic colors to separate user messages, agent
messages, thoughts, tools, system notes and read-only requests; thinking and
tool details collapse behind a shared disclosure. All agent text stays plain
selectable text with inert URLs and no embedded actions.

## Rendering ownership

The native reducer still owns styled history. `TerminalRuntimeStore` subscribes
alongside it before connecting and separately bounds raw byte sequences and
host-provided alternate-screen grids. `SessionScreen` renders normal history
through `TerminalView`, Agent conversation through `AgentConversation`, and
raw/alternate/resync states through `WasmTerminalView`.

`runtime/` embeds the small desktop `dragent_terminal_wasm` parser, not the full
19MB web-sharing app. A renderer-local canvas uses emulator cell positions,
the selected desktop palette and the host's authoritative styled grid. It never
uses the prototype WASM `mount()` UI and receives no gateway or credentials.
The native transport remains the input authority; resize requires a connected
same-account controller. The WebView/iframe owns no socket and cannot navigate
to arbitrary URLs.

Raw output is accepted only as a contiguous sequence from the initial byte.
Gaps/overflow show explicit resync copy instead of feeding a partial VT stream;
styled snapshots then take priority without displaying output twice. A fresh
connection epoch resets parser state. The local bridge has one acknowledged
packet in flight, coalesces later changes, and rejects stale document/epoch
messages. `pnpm terminal:sync` regenerates the embedded glue/WASM/runtime after
source changes, and `pnpm terminal:check` detects drift.

Scrolling is drawn from row pictures. `browserRuntime.js` keeps every drawn
row on a ring of offscreen canvases, keyed by a line id that survives new
output (absolute line plus evicted count), and reads only rows it lacks with
the compact `rows_compact`/`view_info` WASM calls instead of a full JSON
snapshot. A history scroll frame is then a couple of blits at a whole-device-
pixel offset that follows the finger, with momentum after a flick. Full-screen
programs get at most one batch of wheel or page keys per frame. The pure parts
(row decoding, run building, momentum, coalescing) live in `renderKit.js`,
which the sync script packages inside the runtime. `pnpm terminal:bench`
measures frame cost in headless Chrome with CPU throttling, against the
working tree or `-- --revision <commit>` (`-- --ab <commit>` interleaves the
two; `-- --renderer 2d|gpu` pins the working tree's renderer).

Where WebGL2 works, `gpuRenderer.js` draws instead, on a canvas laid over the
screen canvas (which keeps touch, focus and accessibility either way). Each
glyph (by text, bold/italic and quarter-pixel phase) is drawn once with the 2D
canvas's own text rendering into an atlas texture that grows to 4096 px and is
emptied when full; wide characters take a box two cells wide and colour glyphs
are kept in colour. Every ring slot owns a fixed region of instance data for
backgrounds, glyphs and decorations, so a frame is four instanced draws,
scrolling changes only uniforms, and output rewrites only the rows it changed.
Rectangles get exact-area antialiasing, like the 2D canvas. A missing WebGL2,
a lost context (until it is restored; after three losses for good) or any GPU
error hands drawing to the 2D renderer at once. The pure parts (colour packing,
placement, atlas packing, instance data, renderer choice) live in `gpuKit.js`.
`terminalDocument(id, { renderer })` can pin either renderer;
`scripts/terminal-gpu-parity.test.ts` screenshots both in headless Chrome and
holds them to a per-tile tolerance (skipped without Chrome;
`TERMINAL_PARITY_OUT=<dir>` keeps the images).

## Touch on the live terminal

A tap never raises the keyboard; the composer under the terminal types, and
its key row sends Esc, Tab and arrows. A tap on a web link (followed across
wrapped rows) asks the app to open it in the browser; only `http`/`https`
links leave the document (`openableLink`). For a program that turned on mouse
reporting, a tap on the live screen is a left click at that cell (SGR or
X10). Touch and hold selects the word under the finger, dragging selects more,
and the menu offers Copy (and Open link when the selection is one); the app
puts the text on the clipboard. None of this sends anything to the desktop
except the click a program asked for.
