//! Async mermaid → SVG rendering via the pure-Rust `mermaid_to_svg` crate.
//!
//! Rendering happens off the main thread (parsing/layout of a large diagram
//! isn't free) but fully in-process — no sidecar, no system webview. Results
//! are cached by a hash of the diagram source so each unique diagram is
//! rendered exactly once. The rendered SVG is displayed with warpui's `Image`
//! element, which rasterizes SVG cross-platform via resvg.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use mermaid_to_svg::{render_mermaid_to_svg, MermaidTheme};
use once_cell::sync::Lazy;

#[derive(Clone)]
pub enum DiagramState {
    /// A background render is in flight.
    Pending,
    /// Rendered SVG written to this temp path, with its intrinsic size (from the
    /// SVG `viewBox`) so the display can preserve aspect ratio.
    Ready {
        path: PathBuf,
        width: f32,
        height: f32,
    },
    /// Render failed; carries a short message.
    Failed(String),
}

/// Extract the intrinsic `(width, height)` from an SVG's `viewBox="minX minY w h"`.
fn parse_viewbox(svg: &str) -> Option<(f32, f32)> {
    let idx = svg.find("viewBox=\"")?;
    let rest = &svg[idx + 9..];
    let end = rest.find('"')?;
    let nums: Vec<f32> = rest[..end]
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    if nums.len() == 4 && nums[2] > 0.0 && nums[3] > 0.0 {
        Some((nums[2], nums[3]))
    } else {
        None
    }
}

static CACHE: Lazy<Mutex<HashMap<u64, DiagramState>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Bumped whenever a background render completes, so the UI can tell that a
/// re-render/repaint would now show a newly-ready diagram.
pub static GENERATION: AtomicU64 = AtomicU64::new(0);

/// True while at least one render is in flight (drives the repaint poll).
pub static PENDING_COUNT: AtomicU64 = AtomicU64::new(0);

fn hash_source(src: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    src.hash(&mut h);
    h.finish()
}

/// Current render state for `source`, kicking off a background render the first
/// time a given diagram is seen. Safe to call every frame.
pub fn get_or_render(source: &str) -> DiagramState {
    let key = hash_source(source);

    {
        let map = CACHE.lock().unwrap();
        if let Some(state) = map.get(&key) {
            return state.clone();
        }
    }

    // Not seen yet — claim it under the lock, then spawn.
    {
        let mut map = CACHE.lock().unwrap();
        if let Some(state) = map.get(&key) {
            return state.clone();
        }
        map.insert(key, DiagramState::Pending);
    }

    PENDING_COUNT.fetch_add(1, Ordering::Relaxed);
    let owned = source.to_string();
    std::thread::spawn(move || {
        let result = render_blocking(&owned, key);
        {
            let mut map = CACHE.lock().unwrap();
            map.insert(key, result);
        }
        PENDING_COUNT.fetch_sub(1, Ordering::Relaxed);
        GENERATION.fetch_add(1, Ordering::Relaxed);
    });

    DiagramState::Pending
}

fn render_blocking(source: &str, key: u64) -> DiagramState {
    let svg = match render_mermaid_to_svg(source, Some(&MermaidTheme::dark())) {
        Ok(svg) => svg,
        Err(e) => return DiagramState::Failed(e.to_string()),
    };
    let (width, height) = parse_viewbox(&svg).unwrap_or((400.0, 300.0));

    let dir = std::env::temp_dir().join("dragent-diagrams");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{key:016x}.svg"));
    if let Err(e) = std::fs::write(&path, svg.as_bytes()) {
        return DiagramState::Failed(format!("write svg: {e}"));
    }
    DiagramState::Ready {
        path,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_simple_flowchart() {
        let state = render_blocking("flowchart TD\n  A[Start] --> B[End]", 0);
        match state {
            DiagramState::Ready {
                path,
                width,
                height,
            } => {
                assert!(width > 0.0 && height > 0.0);
                let svg = std::fs::read_to_string(&path).unwrap();
                assert!(svg.contains("<svg"));
                let _ = std::fs::remove_file(&path);
            }
            DiagramState::Failed(e) => panic!("expected Ready, got Failed({e})"),
            DiagramState::Pending => panic!("expected Ready, got Pending"),
        }
    }
}
