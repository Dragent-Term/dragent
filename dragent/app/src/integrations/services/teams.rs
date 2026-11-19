//! Microsoft Teams messages, through Microsoft Graph with a browser
//! sign-in.

use serde_json::{json, Value};

use crate::integrations::items::{clip, first_line, slug, Item};
use crate::integrations::oauth::{access_token, MICROSOFT};
use crate::integrations::{http, read_json, Connection, Provider};

const GRAPH: &str = "https://graph.microsoft.com/v1.0";

pub(super) fn verify(connection: &mut Connection) -> Result<Option<String>, String> {
    let token = access_token(connection, &MICROSOFT)?;
    let response = http()?.get(format!("{GRAPH}/me")).bearer_auth(token).send();
    let me = read_json(Provider::Teams, response)?;
    Ok(super::named(
        me["displayName"].as_str(),
        me["mail"]
            .as_str()
            .or_else(|| me["userPrincipalName"].as_str()),
    ))
}

pub(super) fn search(connection: &mut Connection, query: &str) -> Result<Vec<Item>, String> {
    // Graph only finds messages for words.
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let token = access_token(connection, &MICROSOFT)?;
    let body = json!({"requests": [{
        "entityTypes": ["chatMessage"],
        "query": {"queryString": query},
        "from": 0,
        "size": 25
    }]});
    let response = http()?
        .post(format!("{GRAPH}/search/query"))
        .bearer_auth(token)
        .json(&body)
        .send();
    let data = read_json(Provider::Teams, response)?;
    Ok(parse(&data))
}

fn parse(data: &Value) -> Vec<Item> {
    data["value"][0]["hitsContainers"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|container| container["hits"].as_array().into_iter().flatten())
        .filter_map(|hit| {
            let resource = &hit["resource"];
            let text = super::strip_html(
                resource["body"]["content"]
                    .as_str()
                    .or_else(|| hit["summary"].as_str())
                    .unwrap_or_default(),
            );
            if text.is_empty() {
                return None;
            }
            let title = first_line(&text, 90);
            Some(Item {
                provider: Provider::Teams,
                key: String::new(),
                url: resource["webLink"]
                    .as_str()
                    .unwrap_or("https://teams.microsoft.com")
                    .to_string(),
                body: clip(&text),
                detail: resource["from"]["emailAddress"]["name"]
                    .as_str()
                    .map(str::to_string),
                branch: format!("teams-{}", slug(&title, 40))
                    .trim_end_matches('-')
                    .to_string(),
                reference: None,
                title,
                state: None,
                assignee: None,
                labels: Vec::new(),
                updated_at_ms: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_hits_read_as_messages() {
        let items = parse(&json!({"value": [{"hitsContainers": [{"hits": [{
            "summary": "ignored",
            "resource": {"body": {"content": "<p>Checkout <b>is down</b></p>"}, "from": {"emailAddress": {"name": "Ada"}}, "webLink": "https://teams.microsoft.com/l/message/1"}
        }]}]}]}));
        assert_eq!(
            (items[0].title.as_str(), items[0].detail.as_deref()),
            ("Checkout is down", Some("Ada"))
        );
        assert_eq!(items[0].branch, "teams-checkout-is-down");
    }
}
