//! Small regression probe for a running development build.
//!
//! Start Dragent with `DRAGENT_AUTOMATION_PORT` and `DRAGENT_AUTOMATION_TOKEN`
//! (32+ characters), then run this binary with the same two variables. Optional environment budgets turn measurements into CI failures.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Report {
    ok: bool,
    process: ProcessStats,
    render: RenderStats,
    content: ContentStats,
}

#[derive(Debug, Deserialize)]
struct ProcessStats {
    current_rss_mb: f64,
    peak_rss_mb: f64,
}

#[derive(Debug, Deserialize)]
struct RenderStats {
    first_frame_ms: Option<f64>,
    frames: u64,
    redraw_requests: u64,
    scene_rebuilds: u64,
    average_frame_ms: f64,
    max_frame_ms: f64,
}

#[derive(Debug, Deserialize)]
struct ContentStats {
    blocks: usize,
    running_blocks: usize,
    buffered_lines: usize,
    buffered_bytes: usize,
}

fn main() -> Result<()> {
    let port = env_number::<u16>("DRAGENT_AUTOMATION_PORT")
        .context("set DRAGENT_AUTOMATION_PORT to the running app's automation port")?;
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    let mut stream = TcpStream::connect_timeout(&address.into(), Duration::from_secs(2))
        .with_context(|| format!("connect to Dragent automation at {address}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    // The same secret the app was started with (DRAGENT_AUTOMATION_TOKEN).
    let token = std::env::var("DRAGENT_AUTOMATION_TOKEN")
        .context("set DRAGENT_AUTOMATION_TOKEN to the token the app was started with")?;
    let auth = serde_json::json!({"cmd": "auth", "token": token}).to_string();
    stream.write_all(format!("{auth}\n").as_bytes())?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut auth_reply = String::new();
    reader.read_line(&mut auth_reply)?;
    if !auth_reply.contains("\"ok\":true") {
        bail!("Dragent refused the automation token");
    }
    stream.write_all(b"{\"cmd\":\"perf_stats\"}\n")?;

    let mut response = String::new();
    let bytes_read = reader.read_line(&mut response)?;
    if bytes_read == 0 || response.trim().is_empty() {
        bail!(
            "the service on port {port} returned no performance data; start the Dragent desktop with DRAGENT_AUTOMATION_PORT={port} first"
        );
    }
    let report: Report = serde_json::from_str(&response).context("decode performance report")?;
    if !report.ok {
        bail!("Dragent rejected the performance probe");
    }

    print_report(&report);
    enforce_budget(
        "current RSS",
        report.process.current_rss_mb,
        "DRAGENT_PERF_MAX_RSS_MB",
    )?;
    if let Some(first_frame_ms) = report.render.first_frame_ms {
        enforce_budget(
            "first frame",
            first_frame_ms,
            "DRAGENT_PERF_MAX_FIRST_FRAME_MS",
        )?;
    }
    Ok(())
}

fn print_report(report: &Report) {
    println!(
        "memory: {:.1} MiB current, {:.1} MiB peak",
        report.process.current_rss_mb, report.process.peak_rss_mb
    );
    println!(
        "render: first={:?} ms avg={:.2} ms max={:.2} ms frames={} redraws={} rebuilds={}",
        report.render.first_frame_ms,
        report.render.average_frame_ms,
        report.render.max_frame_ms,
        report.render.frames,
        report.render.redraw_requests,
        report.render.scene_rebuilds,
    );
    println!(
        "content: blocks={} running={} lines={} bytes={}",
        report.content.blocks,
        report.content.running_blocks,
        report.content.buffered_lines,
        report.content.buffered_bytes,
    );
}

fn enforce_budget(label: &str, actual: f64, variable: &str) -> Result<()> {
    let Some(maximum) = env_number::<f64>(variable) else {
        return Ok(());
    };
    if actual > maximum {
        bail!("{label} regression: {actual:.2} exceeds {maximum:.2} ({variable})");
    }
    Ok(())
}

fn env_number<T>(name: &str) -> Option<T>
where
    T: std::str::FromStr,
{
    std::env::var(name).ok()?.parse().ok()
}
