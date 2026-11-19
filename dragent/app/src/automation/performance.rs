use std::sync::Arc;

use parking_lot::FairMutex;
use serde_json::{json, Value};

use crate::terminal::model::TerminalSessionCore;

/// Capture expensive content counts only when the loopback development probe
/// asks for them. Frame-path instrumentation itself remains allocation-free.
pub(super) fn snapshot(core: &Arc<FairMutex<TerminalSessionCore>>) -> Value {
    let render = runtime_metrics::render_snapshot();
    let memory = runtime_metrics::process_memory();
    let core = core.lock();
    let blocks = core.blocks().blocks();
    let buffered_lines = blocks.iter().map(|block| block.lines.len()).sum::<usize>();
    let buffered_bytes = blocks
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| line.plain_text().len())
        .sum::<usize>();
    let running_blocks = blocks.iter().filter(|block| block.is_running()).count();

    json!({
        "ok": true,
        "process": {
            "current_rss_mb": memory.current_rss_mb,
            "peak_rss_mb": memory.peak_rss_mb,
        },
        "render": {
            "uptime_ms": render.uptime_ms,
            "first_frame_ms": render.first_frame_ms,
            "frames": render.frames,
            "redraw_requests": render.redraw_requests,
            "scene_rebuilds": render.scene_rebuilds,
            "average_frame_ms": render.average_frame_ms,
            "max_frame_ms": render.max_frame_ms,
            "last_frame_age_ms": render.last_frame_age_ms,
        },
        "terminal_stages": crate::terminal::performance::snapshot(),
        "content": {
            "blocks": blocks.len(),
            "running_blocks": running_blocks,
            "buffered_lines": buffered_lines,
            "buffered_bytes": buffered_bytes,
        }
    })
}
