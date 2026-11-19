//! Doom-loop guard: trip when the model repeats the identical tool call.
//!
//! A model stuck in a loop re-issues the same tool name with the same
//! arguments turn after turn, burning quota while making no progress.
//! Previously nothing client-side caught this; now the third consecutive
//! identical call is answered with a guidance error instead of executing,
//! telling the model to change approach or synthesize from evidence.
//!
//! Operates on the controller's per-turn call ledger (`turn_tool_calls`,
//! cleared on every new turn, candidate call already appended), so the
//! window is exactly one turn: identical calls across separate turns are
//! legitimate retries, not loops.

use crate::types::TurnToolCall;

/// Consecutive identical (name + args) calls that trip the guard.
pub const DOOM_LOOP_THRESHOLD: usize = 3;

/// True when the trailing `DOOM_LOOP_THRESHOLD` entries of `history`
/// (oldest first, latest candidate call already appended) are identical in
/// name and arguments.
pub fn doom_loop_tripped(history: &[TurnToolCall]) -> bool {
    if history.len() < DOOM_LOOP_THRESHOLD {
        return false;
    }
    let last = &history[history.len() - 1];
    history
        .iter()
        .rev()
        .take(DOOM_LOOP_THRESHOLD)
        .all(|c| c.name == last.name && c.args_json == last.args_json)
}

/// Guidance error posted as the trip result. Names the stuck call and points
/// at the two productive exits: vary the approach or stop calling tools.
pub fn doom_loop_message(name: &str, occurrences: usize) -> String {
    format!(
        "Loop guard: '{name}' with identical arguments was already issued \
         {occurrences} times in a row with no progress. Do NOT call it again \
         unchanged — change the arguments, try a different tool, or synthesize \
         the evidence gathered so far into the final response now."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: &str) -> TurnToolCall {
        TurnToolCall {
            call_id: "id".to_string(),
            name: name.to_string(),
            args_json: args.to_string(),
            result_summary: None,
            is_error: false,
        }
    }

    #[test]
    fn trips_on_third_identical_call() {
        let history = vec![
            call("grep", "{\"p\":\"x\"}"),
            call("grep", "{\"p\":\"x\"}"),
            call("grep", "{\"p\":\"x\"}"),
        ];
        assert!(doom_loop_tripped(&history));
    }

    #[test]
    fn no_trip_below_threshold() {
        assert!(!doom_loop_tripped(&[]));
        assert!(!doom_loop_tripped(&[call("grep", "{\"p\":\"x\"}")]));
        assert!(!doom_loop_tripped(&[
            call("grep", "{\"p\":\"x\"}"),
            call("grep", "{\"p\":\"x\"}"),
        ]));
    }

    #[test]
    fn interleaved_calls_reset_the_window() {
        let history = vec![
            call("grep", "{\"p\":\"x\"}"),
            call("read_file", "{\"path\":\"a\"}"),
            call("grep", "{\"p\":\"x\"}"),
        ];
        assert!(!doom_loop_tripped(&history));
    }

    #[test]
    fn same_tool_different_args_is_not_a_loop() {
        let history = vec![
            call("grep", "{\"p\":\"x\"}"),
            call("grep", "{\"p\":\"y\"}"),
            call("grep", "{\"p\":\"y\"}"),
        ];
        assert!(!doom_loop_tripped(&history));
    }

    #[test]
    fn message_names_the_call() {
        let msg = doom_loop_message("run_command", 3);
        assert!(msg.contains("run_command"));
        assert!(msg.contains("Do NOT call it again"));
    }
}
