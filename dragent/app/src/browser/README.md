# Browser design mode

Open a page in a browser pane and click **Design** in the pane's bar, or press Cmd/Ctrl+Shift+D.

- **Pick.** Hovering highlights the element under the pointer with its tag, component, and size. Click it to make a note: a card opens beside the element with a field for the requested change and a kind (Change, Fix, Question, Remove). Enter saves the note and picking carries on, so you can point at several elements in a row. Alt+Up selects the parent before you click; Alt+Down selects the first child. The click never activates the page element.
- **Copy without a note.** Press C while hovering to copy the element's HTML, CSS, and location. A note's card can copy its own context or its screenshot.
- **Draw.** **Draw** in the bar captures the page. Mark it up with arrows, boxes, freehand, highlighter, or text in a choice of colours and widths, with undo and redo. **Add note** turns the marked-up screenshot into a note.
- **Review.** Numbered pins mark each note's element; click one to edit or delete it. The notes button at the bottom right lists every note, including drawings and notes made on other pages.
- **Send.** The agent chip in the bar opens the same agent menu as diff comments and the prompt screen (search, agent icons): sessions already running in this workspace first, then installed agents that would start a new session, opening where diff comments open theirs. Until you pick, it follows the session you last chose, else the most recently active running session. **Send N** (or Cmd/Ctrl+Enter) delivers every note: pasted into a running session, or as the new session's prompt. Sending is only possible from the app, never from the page.
- **Escape** closes a card (a new note with nothing written is discarded), then leaves design mode.

The agent receives a short brief: each request with the element, its source file when the page exposes one, its selector, and the screenshot path. The full context (DOM, computed CSS, accessible role and name, ancestor path, nearby text) is written to a file the brief points to, so a terminal agent's input isn't flooded. Dragent's own agent also receives the screenshots as attachments.

The overlay takes its colours from the current Dragent theme and follows theme changes. Drafts belong to the browser pane and survive reloads and app restarts. Sending removes only the delivered notes; a failed delivery keeps them. Drafts, screenshots, and context files live under the organisation's `browser-design/<pane-id>` directory.

Picking runs only while design mode is open. Pointer work is coalesced to animation frames; DOM extraction, note count, image decoding, drawing history, and stored image size are bounded. Cropping, thumbnails, and file writes run off the UI thread. Closing design mode removes its document listeners and overlays.

Source hints depend on development metadata exposed by the application (React `_debugSource` and React 19 debug stacks, Vue `__file`, Svelte dev metadata, `data-source`, `data-inspector-*`); they are not guaranteed in production builds. A cross-origin iframe is selected as a frame, since the parent page cannot inspect its document. Captures use WKWebView on macOS, WebView2 on Windows, and WebKitGTK on Linux. Linux browser panes currently require X11.

Validation:

```sh
cargo test -p dragent --lib browser:: -j 1
DRAGENT_PLAYWRIGHT_MODULE=/path/to/playwright-core \
DRAGENT_CHROME=/path/to/chrome \
node --test app/tests/browser_design.mjs
DRAGENT_BINARY=target/debug/dragent python3 app/tests/browser_design_native.py
```

The JavaScript checks use a single headless browser and a local fixture; one of them drives the overlay with real pointer input through its closed shadow root, as shipped. They do not open the user's browser profile or submit prompts to a real agent.
