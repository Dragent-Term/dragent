//! §3.2 dependency assertions, run as part of `cargo test -p dragent_e2ee`:
//! snow's features are exactly the pinned set, and getrandom 0.3.4 is the only
//! getrandom linked. The logic lives in `scripts/check-deps.sh` so CI can run
//! it directly too.

#[cfg(unix)]
#[test]
fn dependency_pins_hold() {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/check-deps.sh");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new("bash")
        .arg(script)
        .env("CARGO", cargo)
        .output()
        .expect("run check-deps.sh");
    assert!(
        out.status.success(),
        "check-deps.sh failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
