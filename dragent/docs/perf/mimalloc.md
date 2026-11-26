# mimalloc experiment (measured 2026-11-23, macOS arm64)

Question: does swapping the global allocator to mimalloc win memory without
regressing speed in the desktop app and the host service?

Answer: **no — tied on memory, tied on speed. Keep the feature OFF.**
It stays available behind `--features mimalloc` for future re-measurement,
off by default in both binaries.

## What was built

- `dragent/app`: new `mimalloc = ["dep:mimalloc"]` feature (off by default;
  optional `mimalloc 0.1` dep, native-only). The allocator line lives in the
  app lib's native-only desktop module rather than `app/src/main.rs`: the
  lib already defines a `#[global_allocator]` (the big-allocation tracer),
  and a binary cannot define a second one — they are compiled mutually
  exclusive (`mimalloc::MiMalloc` when the feature is on, the tracer
  otherwise). The wasm viewer never sets the feature.
- `dragent/crates/dragent_host`: same off-by-default feature; the
  `#[global_allocator]` is in the host bin's `main.rs` (the host lib
  defines none, so no conflict there).

## How it was measured (this Mac, 16 GB, shared with another worker)

- Dev builds with `--features performance-harness` (the automation-driven
  load needs it), with and without `dragent/mimalloc`; binaries copied
  aside so later builds could not swap them mid-run.
- Per config, 3 runs each of:
  - **Idle GUI + host memory**: dev app opened with a scratch home
    (created and deleted per run), 4 background `yes | head -n 3000000`
    jobs submitted through the automation server into the one dev-terminal
    session, then `phys_footprint` (`/usr/bin/footprint`) of the GUI and
    host-service PIDs at t = 60 s.
  - **In-app cold start**: launch → first presented frame (`startup
    ready_ms=… frame=first-presented` in the dev log), 3 launches.
  - **Headless process-init probe**: `--bench-probe-startup` (process init
    only, no window).
- Free memory stayed above 3 GB throughout (1.5 GB guard never tripped).

## Results

| measure | default (system allocator) | mimalloc | verdict |
|---|---|---|---|
| GUI idle phys_footprint (3 runs) | 186 / 205 / 222 MB (mean 204) | 195 / 213 / 191 MB (mean 200) | tie (run-to-run noise ±20 MB) |
| host service phys_footprint (3 runs) | 8.4 / 8.2 / 8.5 MB (mean 8.4) | 9.5 / 9.5 / 9.1 MB (mean 9.4) | ~1 MB higher with mimalloc |
| in-app cold start, launch → first frame (3 runs) | 1638 / 594 / 762 ms (median 762) | 673 / 775 / 968 ms (median 775) | tie (machine-load noise dominates) |
| process-init probe (4 runs) | 5 / 0 / 0 / 0 ms | 1 / 0 / 0 / 0 ms | tie (noise floor) |

GUI memory matches the t10 baseline (~190 MB) on both configs. mimalloc
does not win memory here and the host service measures slightly higher, so
per the keep-only-if-it-wins rule the feature stays off.

## Explicitly not measured

- **Release binary size**: skipped on purpose — release builds would add
  several GB to the shared target folder for a number that cannot change
  this decision (memory tied, host higher). If revisited, compare
  `target/release/dragent` + `target/release/dragent-host` sizes.
- **Windows / Linux**: totally unmeasured. Before turning this on anywhere:
  repeat the idle-memory + cold-start table on each OS (allocator behaviour
  is platform-specific; mimalloc's Windows story in particular needs its
  own numbers), plus a release-size check per platform.
- Frame intervals / input latency under load were not re-run per config;
  the cold-start + probe numbers above are the speed signal, and both are
  tied. A future re-measurement that shows a memory win should confirm no
  frame-time regression with the `bench.frame` aggregates before enabling.
