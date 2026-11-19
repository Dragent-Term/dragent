//! Hook bus (opencode `plugin.trigger` parity at the API level).
//!
//! Named trigger points where observers can inspect — and, where documented,
//! rewrite — the agent's inputs and outputs: tool arguments before execution,
//! tool results after, and the assembled system prompt. Subscribers are plain
//! Rust closures today (dynamic plugin loading is staged follow-up work); the
//! bus, payload shapes, and isolation semantics are already the ones external
//! plugins will use.
//!
//! Isolation: one panicking subscriber must never kill a turn, so each call
//! is `catch_unwind`-wrapped and failures are logged, not propagated.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Trigger points. Names mirror opencode's hook vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HookName {
    /// Before a local tool executes. Payload: `{"tool", "args"}` — `args`
    /// rewrites are applied to the call.
    ToolExecuteBefore,
    /// After a local tool executes. Payload: `{"tool", "output", "is_error"}` —
    /// `output` rewrites are applied to what flows onward.
    ToolExecuteAfter,
    /// After the system prompt is assembled. Payload: `{"system"}` — rewrites
    /// are applied to the prompt sent for this turn.
    SystemBuilt,
    /// A user message is about to start a turn. Payload: `{"text"}`.
    /// Reserved: not yet triggered anywhere (staged with plugin loading).
    ChatMessage,
}

impl HookName {
    pub fn as_str(&self) -> &'static str {
        match self {
            HookName::ToolExecuteBefore => "tool.execute.before",
            HookName::ToolExecuteAfter => "tool.execute.after",
            HookName::SystemBuilt => "system.built",
            HookName::ChatMessage => "chat.message",
        }
    }
}

type HookFn = Box<dyn Fn(&mut serde_json::Value) + Send + Sync>;

#[derive(Default)]
pub struct HookBus {
    subs: HashMap<&'static str, Vec<HookFn>>,
}

impl HookBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        name: HookName,
        f: impl Fn(&mut serde_json::Value) + Send + Sync + 'static,
    ) {
        self.subs
            .entry(name.as_str())
            .or_default()
            .push(Box::new(f));
    }

    pub fn subscriber_count(&self, name: HookName) -> usize {
        self.subs.get(name.as_str()).map_or(0, Vec::len)
    }

    /// Run every subscriber for `name` in registration order. A panicking
    /// subscriber is logged and skipped; payload mutations made before the
    /// panic are kept (same as opencode's sequential reducer).
    pub fn trigger(&self, name: HookName, payload: &mut serde_json::Value) {
        let Some(subs) = self.subs.get(name.as_str()) else {
            return;
        };
        for sub in subs {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sub(payload);
            }));
            if result.is_err() {
                log::warn!("[hooks] subscriber panicked on {}", name.as_str());
            }
        }
    }

    /// Process-global bus used by the trigger sites below.
    pub fn global() -> &'static Mutex<HookBus> {
        static BUS: OnceLock<Mutex<HookBus>> = OnceLock::new();
        BUS.get_or_init(|| Mutex::new(HookBus::new()))
    }
}

/// Trigger `name` on the global bus. No-op when nobody subscribed.
pub fn trigger(name: HookName, payload: &mut serde_json::Value) {
    let Ok(bus) = HookBus::global().lock() else {
        return;
    };
    bus.trigger(name, payload);
}

/// Register a subscriber on the global bus (built-ins, tests, future plugins).
pub fn register_global(name: HookName, f: impl Fn(&mut serde_json::Value) + Send + Sync + 'static) {
    if let Ok(mut bus) = HookBus::global().lock() {
        bus.register(name, f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn hook_names_match_opencode_vocabulary() {
        assert_eq!(HookName::ToolExecuteBefore.as_str(), "tool.execute.before");
        assert_eq!(HookName::ToolExecuteAfter.as_str(), "tool.execute.after");
        assert_eq!(HookName::SystemBuilt.as_str(), "system.built");
        assert_eq!(HookName::ChatMessage.as_str(), "chat.message");
    }

    #[test]
    fn trigger_applies_subscriber_mutations_in_order() {
        let mut bus = HookBus::new();
        bus.register(HookName::SystemBuilt, |p| {
            p["system"] =
                serde_json::Value::String(format!("{} [a]", p["system"].as_str().unwrap_or("")));
        });
        bus.register(HookName::SystemBuilt, |p| {
            p["system"] =
                serde_json::Value::String(format!("{}[b]", p["system"].as_str().unwrap_or("")));
        });
        let mut payload = serde_json::json!({"system": "base"});
        bus.trigger(HookName::SystemBuilt, &mut payload);
        assert_eq!(payload["system"], "base [a][b]");
    }

    #[test]
    fn trigger_without_subscribers_is_a_no_op() {
        let bus = HookBus::new();
        let mut payload = serde_json::json!({"tool": "read_file"});
        bus.trigger(HookName::ToolExecuteAfter, &mut payload);
        assert_eq!(payload, serde_json::json!({"tool": "read_file"}));
    }

    #[test]
    fn panicking_subscriber_does_not_break_the_chain() {
        let mut bus = HookBus::new();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let seen_clone = seen.clone();
        bus.register(HookName::ToolExecuteBefore, move |_| {
            seen_clone.lock().unwrap().push("first");
            panic!("boom");
        });
        let seen_clone = seen.clone();
        bus.register(HookName::ToolExecuteBefore, move |_| {
            seen_clone.lock().unwrap().push("second");
        });
        let mut payload = serde_json::json!({});
        bus.trigger(HookName::ToolExecuteBefore, &mut payload);
        assert_eq!(*seen.lock().unwrap(), vec!["first", "second"]);
    }
}
