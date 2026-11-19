//! What a session lets the person choose: model, mode and reasoning effort.
//!
//! Agents report these two ways. Newer ones list "config options", each with
//! a category (`model`, `mode`, `thought_level`) and set by id through
//! `session/set_config_option`. Older ones report `models` and `modes`
//! objects, set through `session/set_model` and `session/set_mode`. Some send
//! both; the config option wins, since it is the one the agent keeps current.

use serde_json::Value;

/// One choice in a picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// How a picker's value is changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetVia {
    /// `session/set_config_option` with this config id.
    ConfigOption(String),
    /// `session/set_model`.
    Model,
    /// `session/set_mode`.
    Mode,
}

/// A list to choose from and what is chosen now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picker {
    pub current: String,
    pub choices: Vec<Choice>,
    pub via: SetVia,
}

impl Picker {
    /// The current choice's name (its id when it isn't listed).
    pub fn current_name(&self) -> &str {
        self.choices
            .iter()
            .find(|c| c.id == self.current)
            .map_or(self.current.as_str(), |c| c.name.as_str())
    }
}

/// Everything a session lets you choose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionOptions {
    pub model: Option<Picker>,
    pub mode: Option<Picker>,
    /// Reasoning effort, when the agent offers it apart from the model.
    pub effort: Option<Picker>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// A `select` config option as a picker. Options may be grouped
/// (`{group, name, options: [...]}`); groups are flattened.
fn config_picker(option: &Value) -> Option<Picker> {
    if option.get("type").and_then(Value::as_str).is_some_and(|t| t != "select") {
        return None;
    }
    let id = text(option, "id")?;
    let current = option.get("currentValue").map(|v| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })?;
    let mut choices = Vec::new();
    fn collect(list: &Value, out: &mut Vec<Choice>) {
        for entry in list.as_array().into_iter().flatten() {
            if let Some(nested) = entry.get("options") {
                collect(nested, out);
                continue;
            }
            let Some(value) = entry.get("value").and_then(Value::as_str) else { continue };
            out.push(Choice {
                id: value.to_string(),
                name: text(entry, "name").unwrap_or_else(|| value.to_string()),
                description: text(entry, "description"),
            });
        }
    }
    collect(option.get("options").unwrap_or(&Value::Null), &mut choices);
    Some(Picker { current, choices, via: SetVia::ConfigOption(id) })
}

impl SessionOptions {
    /// Read them from a `session/new` (or `session/load`) result, or from a
    /// `config_option_update`'s `configOptions` wrapped as `{configOptions}`.
    pub fn parse(result: &Value) -> Self {
        let mut options = SessionOptions::default();
        for option in result.get("configOptions").and_then(Value::as_array).into_iter().flatten() {
            let slot = match option.get("category").and_then(Value::as_str) {
                Some("model") => &mut options.model,
                Some("mode") => &mut options.mode,
                Some("thought_level") => &mut options.effort,
                // Uncategorised: recognise the common ids.
                None => match option.get("id").and_then(Value::as_str) {
                    Some("model") => &mut options.model,
                    Some("mode") => &mut options.mode,
                    Some("effort" | "reasoning_effort" | "thought_level") => &mut options.effort,
                    _ => continue,
                },
                _ => continue,
            };
            if slot.is_none() {
                *slot = config_picker(option);
            }
        }
        if options.model.is_none() {
            if let Some(models) = result.get("models") {
                let choices: Vec<Choice> = models
                    .get("availableModels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let id = text(m, "modelId")?;
                        Some(Choice { name: text(m, "name").unwrap_or_else(|| id.clone()), id, description: text(m, "description") })
                    })
                    .collect();
                let current = text(models, "currentModelId").or_else(|| choices.first().map(|c| c.id.clone()));
                if let (Some(current), false) = (current, choices.is_empty()) {
                    options.model = Some(Picker { current, choices, via: SetVia::Model });
                }
            }
        }
        if options.mode.is_none() {
            if let Some(modes) = result.get("modes") {
                let choices: Vec<Choice> = modes
                    .get("availableModes")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let id = text(m, "id")?;
                        Some(Choice { name: text(m, "name").unwrap_or_else(|| id.clone()), id, description: text(m, "description") })
                    })
                    .collect();
                let current = text(modes, "currentModeId").or_else(|| choices.first().map(|c| c.id.clone()));
                if let (Some(current), false) = (current, choices.is_empty()) {
                    options.mode = Some(Picker { current, choices, via: SetVia::Mode });
                }
            }
        }
        options
    }

    /// The pickers, by the way they're set (to find which one a change is for).
    pub fn picker_mut(&mut self, via: &SetVia) -> Option<&mut Picker> {
        [&mut self.model, &mut self.mode, &mut self.effort]
            .into_iter()
            .flatten()
            .find(|p| &p.via == via)
    }
}
