//! Read-only smoke check for the local or CI GitHub CLI installation.
//! cargo run -p dragent_host --example pull_request_read -- /path/to/repo [PR_NUMBER]
use dragent_host::pull_requests::{execute, Operation, Request};
fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .ok_or("Pass a repository directory")?;
    let read = |operation| {
        execute(Request {
            root: root.clone(),
            operation,
        })
    };
    let list = read(Operation::List {
        state: "all".into(),
        search: String::new(),
        limit: 1,
    })?;
    let list: serde_json::Value = serde_json::from_str(&list).map_err(|e| e.to_string())?;
    println!(
        "PR discovery: {} sample result(s)",
        list.as_array().map_or(0, Vec::len)
    );
    let number = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .or_else(|| list[0]["number"].as_u64());
    read(Operation::Repository)?;
    println!("Repository policy: OK");
    if let Some(number) = number {
        for (name, operation) in [
            ("Details", Operation::Detail { number }),
            ("Files", Operation::Files { number }),
            ("Threads", Operation::Threads { number }),
            ("Commits", Operation::Commits { number }),
            ("Conversation", Operation::Conversation { number }),
            ("Reviews", Operation::Reviews { number }),
        ] {
            let output = read(operation)?;
            let _: serde_json::Value =
                serde_json::from_str(&output).map_err(|e| format!("{name}: {e}"))?;
            println!("{name}: OK");
        }
    }
    Ok(())
}
