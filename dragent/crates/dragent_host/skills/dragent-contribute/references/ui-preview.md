# UI preview tool

Source: `dragent/app/src/ui_preview.rs`, `dragent/app/src/ui_preview/demo.rs`,
example `dragent/app/examples/ui_preview.rs`. It draws chosen screens with
sample data and saves each as a PNG without running Dragent itself: no
sign-in, no keychain, no workspaces, nothing written but the images. The
file header documents the invocation:

```sh
cargo run -p dragent --example ui_preview -- <folder>
```

Verified in this checkout: the files exist (`ls app/src/ui_preview*`
shows `ui_preview.rs` and `ui_preview/`), and
`cargo check -p dragent --example ui_preview` (shared target variables as
in `workflow.md`) compiles it. A live run needs a display and opens a
window; do not run it headless expecting screenshots.

## At most one preview process at a time

The preview owns its window and output folder. Starting a second one while
the first runs splits screenshots across folders and fights for the
display. Before starting, check:

```sh
ps aux | grep '[u]i_preview' || echo "no preview running"
ls app/src/ui_preview* app/examples/ui_preview.rs
```

Start one, let it finish writing `<folder>`, then stop it (Ctrl-C) before
starting another. Useful filters from the source (`DRAGENT_PREVIEW_ONLY`
prefix match, `DRAGENT_DEMO_CAPTURE=1` for capture mode) are documented in
`app/src/ui_preview.rs`; read that file before using a variable you have
not run. For Code reviews screens the guide documents:

```sh
DRAGENT_DEMO_CAPTURE=1 DRAGENT_PREVIEW_PR_ONLY=1 cargo run -p dragent --example ui_preview -- /tmp/dragent-pr-preview
```

A screenshot proves layout only. Exercise the handler or end-to-end path
as well: a passing preview plus an untested handler is not verification.
