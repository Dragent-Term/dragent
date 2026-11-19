//! Pure `ask_user` / `todowrite` payload parsing.
//!
//! Moved verbatim from `app/src/agent/controller/questions.rs` (pure
//! serde_json mapping, zero WarpUI deps). The app re-exports these paths.

use crate::block::{QuestionItem, QuestionOption, TodoListItem};

pub fn parse_user_questions(args_json: &str) -> Result<Vec<QuestionItem>, String> {
    let v: serde_json::Value =
        serde_json::from_str(args_json).map_err(|e| format!("invalid JSON: {e}"))?;
    let raw = v
        .get("questions")
        .and_then(|q| q.as_array())
        .ok_or("missing `questions` array")?;

    let mut questions = Vec::new();
    for q in raw {
        let question = q
            .get("question")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim();
        if question.is_empty() {
            continue;
        }
        let header = q
            .get("header")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let multi_select = q
            .get("multiSelect")
            .or_else(|| q.get("multi_select"))
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        // Free-text answers stay available unless the model opts out, so a
        // strict choice can be forced per question (like opencode's `custom`).
        let custom = q.get("custom").and_then(|b| b.as_bool()).unwrap_or(true);

        let mut options = Vec::new();
        if let Some(opts) = q.get("options").and_then(|o| o.as_array()) {
            for o in opts {
                if let Some(label) = o.as_str() {
                    // Bare-string option.
                    let label = label.trim();
                    if !label.is_empty() {
                        options.push(QuestionOption {
                            label: label.to_string(),
                            description: String::new(),
                        });
                    }
                } else if let Some(label) = o.get("label").and_then(|s| s.as_str()) {
                    let label = label.trim();
                    if label.is_empty() {
                        continue;
                    }
                    let description = o
                        .get("description")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    options.push(QuestionOption {
                        label: label.to_string(),
                        description,
                    });
                }
            }
        }
        // Need at least two distinct choices for a meaningful question.
        if options.len() < 2 {
            continue;
        }
        questions.push(QuestionItem {
            header,
            question: question.to_string(),
            options,
            multi_select,
            custom,
        });
    }

    if questions.is_empty() {
        return Err("no valid questions (each needs `question` text and 2+ options)".to_string());
    }
    // Cap to keep the interaction bounded regardless of what the model sends.
    questions.truncate(4);
    Ok(questions)
}

/// Clamp the keyboard cursor after a move: wraps within `0..len`, and stays
/// at 0 when there are no options to point at.
pub fn clamp_question_cursor(cursor: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = cursor as i32 + delta;
    next.rem_euclid(len as i32) as usize
}

/// Apply a click to a question's answer slot. Multi-select toggles the option
/// and returns false (wait for an explicit Continue); single-select replaces
/// the answer and returns true (advance immediately).
pub fn apply_option_pick(multi_select: bool, slot: &mut Vec<String>, option: &str) -> bool {
    if multi_select {
        if let Some(pos) = slot.iter().position(|s| s == option) {
            slot.remove(pos);
        } else {
            slot.push(option.to_string());
        }
        false
    } else {
        *slot = vec![option.to_string()];
        true
    }
}

/// Apply a typed custom ("Other") answer. Single-select replaces the answer and
/// advances; multi-select appends it (deduped) and stays for more picks.
pub fn apply_text_answer(multi_select: bool, slot: &mut Vec<String>, text: &str) -> bool {
    if multi_select {
        if !slot.iter().any(|s| s == text) {
            slot.push(text.to_string());
        }
        false
    } else {
        *slot = vec![text.to_string()];
        true
    }
}

/// Resolve a reply typed in the main input against a question's options.
/// Each comma-separated part may be a 1-based option number ("1", "2.", "3)")
/// or an option label (case-insensitive). If every part resolves, the matched
/// labels are returned (deduped); otherwise the whole text is one free-text
/// answer — never silently dropped.
pub fn resolve_typed_answer(text: &str, options: &[QuestionOption]) -> Vec<String> {
    let resolve_one = |part: &str| -> Option<String> {
        let trimmed = part.trim().trim_end_matches(['.', ')']);
        if let Ok(n) = trimmed.parse::<usize>() {
            if n >= 1 && n <= options.len() {
                return Some(options[n - 1].label.clone());
            }
            return None;
        }
        options
            .iter()
            .find(|o| o.label.eq_ignore_ascii_case(part.trim()))
            .map(|o| o.label.clone())
    };

    let parts: Vec<&str> = text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut picks = Vec::new();
    for part in &parts {
        match resolve_one(part) {
            Some(label) => {
                if !picks.contains(&label) {
                    picks.push(label);
                }
            }
            None => return vec![text.to_string()], // free-text answer, verbatim
        }
    }
    if picks.is_empty() {
        return vec![text.to_string()];
    }
    picks
}

/// Serialize the user's answers as the tool result the agent reads back.
/// One line per answered question ("Asked: … → Answered: …") from the posted
/// `ask_user` result JSON — the human-readable digest hosts keep as memory.
pub fn summarize_question_answers(answers_json: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(answers_json) else {
        return String::new();
    };
    value["answers"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let question = entry["question"].as_str()?;
                    let selected: Vec<&str> = entry["selected"]
                        .as_array()
                        .map(|s| s.iter().filter_map(|v| v.as_str()).collect())
                        .unwrap_or_default();
                    Some(format!(
                        "Asked: {question} → Answered: {}",
                        selected.join(", ")
                    ))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Shape: `{"answers":[{"header","question","selected":[...]}], "instruction": ...}`.
/// The `instruction` is a nudge at the exact point the model reads the answer, so
/// it CONTINUES the task instead of stopping to acknowledge the choice.
pub fn build_question_result_json(questions: &[QuestionItem], answers: &[Vec<String>]) -> String {
    let entries: Vec<serde_json::Value> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let selected = answers.get(i).cloned().unwrap_or_default();
            // Options are echoed so the model can interpret the answer even
            // when it is free text referencing them ("the second one", etc.).
            let options: Vec<&str> = q.options.iter().map(|o| o.label.as_str()).collect();
            serde_json::json!({
                "header": q.header,
                "question": q.question,
                "options": options,
                "selected": selected,
            })
        })
        .collect();
    serde_json::json!({
        "answers": entries,
        "instruction": "You now have the user's decision(s). Note: `selected` \
            entries may be free-form text rather than one of the offered options — \
            the user may explain WHY they chose an option, qualify it with \
            conditions, or describe something different they actually want. Read \
            the text carefully and honor it over the option list when they \
            diverge. CONTINUE the task in this same turn using these answers — \
            take the next concrete action (read/edit/run tools as needed) right \
            now. Do NOT stop, and do NOT merely restate what you will do or ask \
            for confirmation, unless there is another genuine decision that \
            requires a new ask_user call."
    })
    .to_string()
}

pub fn normalize_todo_status(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "pending" => "pending",
        "in_progress" => "in_progress",
        "completed" => "completed",
        "cancelled" | "canceled" => "cancelled",
        _ => "pending",
    }
    .to_string()
}

/// Normalize a model-supplied todo priority; unknown values default to medium.
pub fn normalize_todo_priority(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "high" => "high",
        "low" => "low",
        _ => "medium",
    }
    .to_string()
}

pub fn parse_todo_list(args_json: &str) -> Result<Vec<TodoListItem>, String> {
    let v: serde_json::Value =
        serde_json::from_str(args_json).map_err(|e| format!("invalid JSON: {e}"))?;
    let raw = v
        .get("todos")
        .and_then(|t| t.as_array())
        .ok_or("missing `todos` array")?;
    let mut todos = Vec::new();
    for t in raw {
        let content = t
            .get("content")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            continue;
        }
        let status = t
            .get("status")
            .and_then(|s| s.as_str())
            .map(normalize_todo_status)
            .unwrap_or_else(|| "pending".to_string());
        let priority = t
            .get("priority")
            .and_then(|s| s.as_str())
            .map(normalize_todo_priority)
            .unwrap_or_else(|| "medium".to_string());
        todos.push(TodoListItem {
            content: content.to_string(),
            status,
            priority,
        });
    }
    Ok(todos)
}
