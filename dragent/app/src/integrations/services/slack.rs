//! Slack's Web API, with a user token from an app the user installed.
//! Slack answers 200 even for failures: `ok` says whether it worked.

use serde_json::Value;

use crate::integrations::items::{clip, first_line, slug, Item, LIMIT};
use crate::integrations::{http, Connection, Provider};

fn get(connection: &Connection, method: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    let data = connection.send(
        http()?
            .get(format!("https://slack.com/api/{method}"))
            .query(query),
    )?;
    if data["ok"].as_bool() != Some(true) {
        let error = data["error"].as_str().unwrap_or("unknown_error");
        return Err(match error {
            "invalid_auth" | "not_authed" | "token_revoked" | "account_inactive" => {
                crate::integrations::expired_message("Slack", false)
            }
            "missing_scope" => format!(
                "Slack {} because the token is missing a scope ({}). Add it to the app, reinstall it, then reconnect with the new token.",
                crate::integrations::REFUSED_REQUEST,
                data["needed"].as_str().unwrap_or("search:read")
            ),
            "not_allowed_token_type" => {
                "Slack needs a user token (xoxp-…), not a bot token, to search messages."
                    .to_string()
            }
            other => format!("Slack: {other}"),
        });
    }
    Ok(data)
}

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let me = get(connection, "auth.test", &[])?;
    let user = me["user"].as_str().map(|user| format!("@{user}"));
    Ok(super::named(me["team"].as_str(), user.as_deref()))
}

pub(super) fn search(connection: &Connection, query: &str) -> Result<Vec<Item>, String> {
    // Slack only finds messages for words.
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let limit = LIMIT.to_string();
    let data = get(
        connection,
        "search.messages",
        &[
            ("query", query),
            ("count", limit.as_str()),
            ("sort", "timestamp"),
        ],
    )?;
    Ok(parse(&data))
}

fn parse(data: &Value) -> Vec<Item> {
    data["messages"]["matches"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| {
            let text = message["text"]
                .as_str()
                .filter(|text| !text.trim().is_empty())?;
            let channel_id = message["channel"]["id"].as_str()?;
            let ts = message["ts"].as_str()?;
            // The thread the message belongs to (its own, when it starts one).
            let thread = message["thread_ts"].as_str().unwrap_or(ts);
            let channel = message["channel"]["name"]
                .as_str()
                .filter(|name| !name.is_empty())
                .map(|name| format!("#{name}"))
                .unwrap_or_default();
            let title = first_line(text, 90);
            Some(Item {
                provider: Provider::Slack,
                key: channel,
                url: message["permalink"].as_str()?.to_string(),
                body: clip(text),
                detail: message["username"].as_str().map(str::to_string),
                branch: format!("slack-{}", slug(&title, 40))
                    .trim_end_matches('-')
                    .to_string(),
                reference: Some(format!("{channel_id}:{thread}")),
                title,
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
            })
        })
        .collect()
}

/// A thread reply, or a new message in a channel (a reference without a
/// thread). Needs the `chat:write` user scope.
fn post(connection: &Connection, method: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    let data = connection.send(
        http()?
            .post(format!("https://slack.com/api/{method}"))
            .form(form),
    )?;
    if data["ok"].as_bool() != Some(true) {
        return Err(match data["error"].as_str().unwrap_or("unknown_error") {
            "missing_scope" => {
                "The Slack token can't post: add the chat:write user scope to the app and reinstall it.".to_string()
            }
            "not_in_channel" | "channel_not_found" => "That Slack channel isn't open to this account.".to_string(),
            other => format!("Slack: {other}"),
        });
    }
    Ok(data)
}

pub(super) fn read(connection: &mut Connection, id: &str) -> Result<Item, String> {
    let (channel, ts) = id
        .split_once(':')
        .ok_or("Slack messages look like CHANNEL:TIMESTAMP.")?;
    let text = details(connection, id)?;
    let url = get(
        connection,
        "chat.getPermalink",
        &[("channel", channel), ("message_ts", ts)],
    )
    .ok()
    .and_then(|data| data["permalink"].as_str().map(str::to_string))
    .unwrap_or_default();
    let title = first_line(
        text.split_once(": ")
            .map_or(text.as_str(), |(_, rest)| rest),
        90,
    );
    Ok(Item {
        provider: Provider::Slack,
        key: String::new(),
        branch: format!("slack-{}", slug(&title, 40))
            .trim_end_matches('-')
            .to_string(),
        title,
        url,
        body: clip(&text),
        detail: None,
        reference: Some(id.to_string()),
        state: None,
        assignee: None,
        labels: Vec::new(),
        updated_at_ms: None,
    })
}

/// A reply in the message's thread.
pub(super) fn comment(connection: &mut Connection, id: &str, text: &str) -> Result<(), String> {
    let (channel, ts) = id
        .split_once(':')
        .ok_or("Slack messages look like CHANNEL:TIMESTAMP.")?;
    post(
        connection,
        "chat.postMessage",
        &[("channel", channel), ("thread_ts", ts), ("text", text)],
    )
    .map(|_| ())
}

/// The whole thread, one message per line with its author.
pub(super) fn details(connection: &mut Connection, reference: &str) -> Result<String, String> {
    let (channel, ts) = reference.split_once(':').ok_or("Unknown Slack message")?;
    let data = get(
        connection,
        "conversations.replies",
        &[("channel", channel), ("ts", ts), ("limit", "50")],
    )?;
    Ok(thread_text(&data))
}

fn thread_text(data: &Value) -> String {
    data["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| {
            let text = message["text"].as_str()?.trim();
            let author = message["user_profile"]["real_name"]
                .as_str()
                .or_else(|| message["username"].as_str())
                .or_else(|| message["user"].as_str())
                .unwrap_or("someone");
            (!text.is_empty()).then(|| format!("{author}: {text}"))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messages_point_at_their_thread() {
        let items = parse(&json!({"messages": {"matches": [{
            "text": "Deploy is failing on main\nsee logs", "ts": "1700.2", "thread_ts": "1700.1",
            "channel": {"id": "C1", "name": "eng"}, "permalink": "https://acme.slack.com/archives/C1/p17002", "username": "ada"
        }]}}));
        assert_eq!(
            (items[0].key.as_str(), items[0].title.as_str()),
            ("#eng", "Deploy is failing on main")
        );
        assert_eq!(items[0].reference.as_deref(), Some("C1:1700.1"));
        assert_eq!(items[0].branch, "slack-deploy-is-failing-on-main");
        let thread = thread_text(&json!({"messages": [
            {"text": "Deploy is failing", "user_profile": {"real_name": "Ada"}},
            {"text": "Looking", "user": "U2"}
        ]}));
        assert_eq!(thread, "Ada: Deploy is failing\n\nU2: Looking");
    }
}
