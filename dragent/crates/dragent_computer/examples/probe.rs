//! Run one computer-use request against this machine:
//! `cargo run -p dragent_computer --example probe -- snapshot '{"app":"Finder"}'`
fn main() {
    let mut args = std::env::args().skip(1);
    let tool = args.next().unwrap_or_else(|| "permissions".into());
    let arguments: serde_json::Value = args
        .next()
        .map(|raw| serde_json::from_str(&raw).expect("arguments must be JSON"))
        .unwrap_or(serde_json::Value::Null);
    let request = dragent_computer::Request::from_tool(&tool, &arguments).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(2)
    });
    let started = std::time::Instant::now();
    match dragent_computer::run(request) {
        Ok(outcome) => println!("{}\n({} ms)", outcome.text, started.elapsed().as_millis()),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1)
        }
    }
}
