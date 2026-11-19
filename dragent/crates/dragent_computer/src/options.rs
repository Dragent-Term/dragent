//! Options shared by observations and actions on a particular window.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TargetOptions {
    #[serde(default)]
    pub window_id: Option<String>,
    #[serde(default)]
    pub window_index: Option<usize>,
    #[serde(default)]
    pub restore_window: bool,
    #[serde(default)]
    pub screenshot: Option<bool>,
}

impl TargetOptions {
    pub fn from_arguments(arguments: &Value) -> Result<Self, String> {
        let mut value = arguments.clone();
        if value.is_null() {
            value = serde_json::json!({});
        }
        if let Some(object) = value.as_object_mut() {
            object.retain(|_, value| !value.is_null());
        }
        if let Some(number) = value["window_id"].as_u64() {
            value["window_id"] = Value::String(number.to_string());
        }
        let mut options: Self =
            serde_json::from_value(value).map_err(|e| format!("Invalid window options: {e}"))?;
        if arguments["no_screenshot"].as_bool() == Some(true) {
            options.screenshot = Some(false);
        }
        if options.window_id.is_some() && options.window_index.is_some() {
            return Err("Choose a window id or a window index, rather than both.".into());
        }
        if options
            .window_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err("Window id cannot be empty.".into());
        }
        if options.window_index == Some(0) {
            return Err("Window indexes start at 1.".into());
        }
        Ok(options)
    }
}
