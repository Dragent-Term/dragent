//! Find in page. The page script marks every match, picks the current one
//! and scrolls it into view; the pane shows "3 of 12" from what it returns.
//!
//! Matches are drawn with CSS custom highlights where the engine has them
//! (Safari 17.2+, Chromium 105+), which leaves the page's own selection and
//! DOM untouched; older engines select the current match instead.

/// Most matches marked in one page; a find for "e" stops counting there.
const MAX_MATCHES: usize = 1000;

/// Where a find stands, as the page reported it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FindResult {
    /// 1-based index of the current match, 0 with none.
    pub current: usize,
    pub total: usize,
}

impl FindResult {
    /// "3 of 12", "No results", or `None` before anything was searched.
    pub fn label(&self, query: &str) -> Option<String> {
        if query.is_empty() {
            return None;
        }
        Some(match self.total {
            0 => "No results".to_string(),
            total if total >= MAX_MATCHES => format!("{} of {MAX_MATCHES}+", self.current),
            total => format!("{} of {total}", self.current),
        })
    }
}

/// Search the page for `query`. `step` 0 starts over from the first match on
/// screen, 1 moves to the next match and -1 to the previous one.
pub fn script(query: &str, step: i8) -> String {
    let query = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(() => {{
  const query = {query}, step = {step}, limit = {MAX_MATCHES};
  const state = window.__dragentFind || (window.__dragentFind = {{ query: null, ranges: [], index: -1 }});
  const marks = typeof CSS !== 'undefined' && CSS.highlights && typeof Highlight === 'function';
  if (marks && !document.getElementById('dragent-find-style')) {{
    const style = document.createElement('style');
    style.id = 'dragent-find-style';
    style.textContent = '::highlight(dragent-find){{background-color:rgba(255,214,0,.45);color:inherit}}' +
      '::highlight(dragent-find-current){{background-color:#ff9632;color:#000}}';
    (document.head || document.documentElement).appendChild(style);
  }}
  if (step === 0 || state.query !== query) {{
    state.query = query; state.ranges = []; state.index = -1;
    const needle = query.toLocaleLowerCase();
    if (needle) {{
      const root = document.body || document.documentElement;
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, {{
        acceptNode(node) {{
          const parent = node.parentElement;
          if (!parent || parent.closest('script,style,noscript,template,[data-dragent-design]')) return NodeFilter.FILTER_REJECT;
          return parent.getClientRects().length ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
        }}
      }});
      for (let node; state.ranges.length < limit && (node = walker.nextNode());) {{
        const lower = node.data.toLocaleLowerCase();
        // Case folding that changes the length can't map back to offsets.
        const text = lower.length === node.data.length ? lower : node.data;
        for (let at = text.indexOf(needle); at !== -1 && state.ranges.length < limit; at = text.indexOf(needle, at + needle.length)) {{
          const range = document.createRange();
          range.setStart(node, at);
          range.setEnd(node, at + needle.length);
          state.ranges.push(range);
        }}
      }}
    }}
  }}
  const total = state.ranges.length;
  if (total) {{
    if (step === 0 || state.index < 0) {{
      const onScreen = state.ranges.findIndex(range => range.getBoundingClientRect().bottom >= 0);
      state.index = onScreen < 0 ? 0 : onScreen;
    }} else {{
      state.index = (state.index + step + total) % total;
    }}
  }}
  const current = total ? state.ranges[state.index] : null;
  if (marks) {{
    CSS.highlights.set('dragent-find', new Highlight(...state.ranges));
    CSS.highlights.set('dragent-find-current', current ? new Highlight(current) : new Highlight());
  }} else if (current) {{
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(current);
  }}
  if (current) {{
    const rect = current.getBoundingClientRect();
    if (rect.top < 0 || rect.bottom > innerHeight) {{
      current.startContainer.parentElement?.scrollIntoView({{ block: 'center', inline: 'nearest' }});
    }}
  }}
  return {{ current: total ? state.index + 1 : 0, total }};
}})()"#
    )
}

/// Remove the marks when the find bar closes.
pub const CLEAR: &str = r#"(() => {
  if (typeof CSS !== 'undefined' && CSS.highlights) {
    CSS.highlights.delete('dragent-find');
    CSS.highlights.delete('dragent-find-current');
  }
  window.__dragentFind = undefined;
})()"#;

/// The page's answer: `{"current":3,"total":12}`, possibly JSON-encoded
/// once more by the engine.
pub fn parse(raw: &str) -> Option<FindResult> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let serde_json::Value::String(inner) = &value {
        value = serde_json::from_str(inner).ok()?;
    }
    Some(FindResult {
        current: value["current"].as_u64()? as usize,
        total: value["total"].as_u64()? as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_is_quoted_into_the_script() {
        let script = script("it's \"quoted\" </script>", 1);
        assert!(script.contains(r#"const query = "it's \"quoted\" </script>", step = 1"#));
    }

    #[test]
    fn reads_the_page_answer_in_either_encoding() {
        let plain = parse(r#"{"current":3,"total":12}"#);
        let wrapped = parse(r#""{\"current\":3,\"total\":12}""#);
        let expected = Some(FindResult { current: 3, total: 12 });
        assert_eq!(plain, expected);
        assert_eq!(wrapped, expected);
        assert_eq!(parse("null"), None);
    }

    #[test]
    fn labels_like_a_desktop_browser() {
        assert_eq!(FindResult::default().label(""), None);
        assert_eq!(FindResult::default().label("x").as_deref(), Some("No results"));
        assert_eq!(FindResult { current: 3, total: 12 }.label("x").as_deref(), Some("3 of 12"));
        assert_eq!(
            FindResult { current: 1, total: MAX_MATCHES }.label("e").as_deref(),
            Some("1 of 1000+")
        );
    }
}
