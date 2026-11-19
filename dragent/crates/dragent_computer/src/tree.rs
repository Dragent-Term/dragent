//! What every platform hands back, and the numbered tree agents read: apps,
//! windows, and a window's elements in tree order, rendered one per line
//! (`[12] button "Save"`). Password fields never show their value, and
//! password managers can't be read or driven at all.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    pub fn is_empty(&self) -> bool {
        self.width < 1.0 || self.height < 1.0
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct AppInfo {
    pub name: String,
    /// Bundle id on macOS, executable name on Windows, the bus name's
    /// program on Linux.
    pub id: String,
    pub pid: u32,
    /// In front, receiving keystrokes.
    pub active: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowInfo {
    /// Stable window identifier for this app process.
    pub id: String,
    #[serde(serialize_with = "serialize_window_index")]
    pub index: usize,
    pub title: String,
    /// Screen points, top-left origin.
    pub frame: Rect,
    pub focused: bool,
    /// The platform's own window id (CGWindowID, HWND), for screenshots.
    #[serde(skip)]
    pub native: u64,
}

/// One element as a platform reads it, in tree (pre-order) order.
pub struct RawNode<H> {
    pub depth: usize,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub description: String,
    pub placeholder: String,
    pub enabled: bool,
    pub focused: bool,
    pub selected: bool,
    pub checked: Option<bool>,
    /// A password field: its value is never shown.
    pub secure: bool,
    pub frame: Option<Rect>,
    pub actions: Vec<String>,
    pub has_children: bool,
    pub handle: H,
}

/// A numbered element of a snapshot.
pub struct Element<H> {
    pub index: usize,
    pub line: String,
    /// `button "Save"`, for confirmations and results.
    pub label: String,
    pub frame: Option<Rect>,
    pub secure: bool,
    pub focused: bool,
    pub actions: Vec<String>,
    pub handle: H,
}

/// Apps whose windows hold secrets: never read or driven.
const BLOCKED: &[&str] = &[
    "1password",
    "bitwarden",
    "dashlane",
    "lastpass",
    "nordpass",
    "proton pass",
    "protonpass",
    "keepass",
    "enpass",
    "roboform",
    "keeper password",
    "keychain access",
    "com.apple.keychainaccess",
    "com.apple.passwords",
];

pub fn is_blocked(app: &AppInfo) -> bool {
    let name = app.name.to_ascii_lowercase();
    let id = app.id.to_ascii_lowercase();
    name == "passwords"
        || BLOCKED
            .iter()
            .any(|blocked| name.contains(blocked) || id.contains(blocked))
}

/// The app an agent named: a bundle id or executable, a name, or `pid:N`;
/// an exact match wins, else a single partial one.
pub fn resolve_app<'a>(apps: &'a [AppInfo], query: &str) -> Result<&'a AppInfo, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Name an app (`dragent computer apps` lists them).".into());
    }
    if let Some(pid) = query
        .strip_prefix("pid:")
        .and_then(|p| p.trim().parse::<u32>().ok())
    {
        return apps
            .iter()
            .find(|app| app.pid == pid)
            .ok_or_else(|| format!("No app is running as pid {pid}."));
    }
    let lower = query.to_ascii_lowercase();
    let exact: Vec<&AppInfo> = apps
        .iter()
        .filter(|app| app.id.eq_ignore_ascii_case(query) || app.name.to_ascii_lowercase() == lower)
        .collect();
    let candidates = if exact.is_empty() {
        apps.iter()
            .filter(|app| {
                app.name.to_ascii_lowercase().contains(&lower)
                    || app.id.to_ascii_lowercase().contains(&lower)
            })
            .collect()
    } else {
        exact
    };
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "No running app matches “{query}”. `dragent computer apps` lists them."
        )),
        many => Err(format!(
            "“{query}” matches {} apps: {}. Use the id or pid:N.",
            many.len(),
            many.iter()
                .take(6)
                .map(|app| format!("{} ({}, pid {})", app.name, app.id, app.pid))
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

/// Roles a person interacts with, worth a number even without a label.
fn interactive(role: &str) -> bool {
    const ROLES: &[&str] = &[
        "button",
        "link",
        "text field",
        "text area",
        "checkbox",
        "check box",
        "radio",
        "menu item",
        "menu button",
        "pop-up",
        "popup",
        "combo box",
        "tab",
        "slider",
        "switch",
        "search field",
        "edit",
        "hyperlink",
        "list item",
        "cell",
        "row",
        "incrementor",
        "spin",
        "toggle",
        "entry",
        "password",
        "document",
        "web area",
    ];
    ROLES.iter().any(|known| role.contains(known))
}

fn clean(text: &str, max: usize) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if one_line.chars().count() > max {
        let cut: String = one_line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        one_line
    }
}

/// Number the elements worth acting on and render one line each, indented
/// by the shown elements above them. Unlabelled containers fold into their
/// children; empty leaves are dropped. Returns whether `max` cut it short.
pub fn build_elements<H>(raw: Vec<RawNode<H>>, max: usize) -> (Vec<Element<H>>, bool) {
    let built = build(raw, max, None);
    (built.elements, built.truncated)
}

/// A window's elements as a snapshot lists them.
pub struct Built<H> {
    pub elements: Vec<Element<H>>,
    /// `max` cut the list short.
    pub truncated: bool,
    /// Parts of web content left out for lying outside the window.
    pub offscreen: usize,
}

/// Content roots of web pages and documents: inside one, whatever lies
/// outside the window is left out until it is scrolled into view.
fn web_root(role: &str) -> bool {
    matches!(role, "web area" | "document")
}

/// Roles that hold a page or window: their title isn't the text inside.
fn page_holder(role: &str) -> bool {
    matches!(
        role,
        "window" | "web area" | "document" | "dialog" | "sheet" | "application"
    )
}

/// Whether `text` under a shown element only repeats that element's label:
/// a link's or button's words, or a labelled group's (pages label table
/// cells that way).
fn repeats(role: &str, label: &str, text: &str) -> bool {
    if text.is_empty() || page_holder(role) {
        return false;
    }
    if matches!(role, "group" | "scroll area") {
        label == text
    } else {
        label.contains(text)
    }
}

/// Roles whose text often sits in their children (a link's words, a row's
/// cells): unlabelled, they take that text as their label.
fn named_by_contents(role: &str) -> bool {
    matches!(
        role,
        "row"
            | "cell"
            | "list item"
            | "data item"
            | "tree item"
            | "table row"
            | "table cell"
            | "link"
            | "button"
            | "heading"
            | "menu item"
            | "tab"
    )
}

/// Actions every element of a web page offers; they don't make one worth a
/// line, and they aren't repeated on every line of text.
fn incidental(action: &str) -> bool {
    matches!(
        action,
        "AXShowMenu" | "AXScrollToVisible" | "AXShowDefaultUI" | "AXShowAlternateUI"
    )
}

fn off_screen(frame: Option<Rect>, view: Rect) -> bool {
    let (left, top) = (view.x, view.y);
    let (right, bottom) = (view.x + view.width, view.y + view.height);
    frame.is_some_and(|f| {
        if f.is_empty() {
            // Browsers squash what's scrolled away into a sliver with no
            // size on the page's edge.
            f.y >= bottom - 1.0
                || f.y + f.height <= top + 1.0
                || f.x >= right - 1.0
                || f.x + f.width <= left + 1.0
        } else {
            f.x + f.width <= left || f.y + f.height <= top || f.x >= right || f.y >= bottom
        }
    })
}

fn overlap(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.x.max(b.x);
    let top = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    let rect = Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };
    (!rect.is_empty()).then_some(rect)
}

/// The web pages around the node being read, innermost last, each with the
/// part of the screen it shows (none without a window to measure against).
struct Pages {
    window: Option<Rect>,
    open: Vec<(usize, Option<Rect>)>,
}

impl Pages {
    /// Move to a node at `depth`, leaving pages it isn't inside.
    fn enter(&mut self, depth: usize) {
        while self.open.last().is_some_and(|(d, _)| *d >= depth) {
            self.open.pop();
        }
    }

    fn inside(&self) -> bool {
        !self.open.is_empty()
    }

    /// Whether a frame lies outside what the page around it shows.
    fn hidden(&self, frame: Option<Rect>) -> bool {
        matches!(self.open.last(), Some((_, Some(view))) if off_screen(frame, *view))
    }

    /// Note a node that is itself a page.
    fn open(&mut self, depth: usize, role: &str, frame: Option<Rect>) {
        if web_root(role) {
            let view = self.window.map(|window| {
                frame
                    .and_then(|frame| overlap(frame, window))
                    .unwrap_or(window)
            });
            self.open.push((depth, view));
        }
    }
}

/// Plain text: shown for what it says, not for anything it does.
fn plain_text<H>(node: &RawNode<H>) -> bool {
    node.role == "text"
        && !node.focused
        && !node.secure
        && node.actions.iter().all(|action| incidental(action))
}

fn spoken<H>(node: &RawNode<H>, max: usize) -> String {
    [
        &node.name,
        node.value.as_deref().unwrap_or_default(),
        &node.description,
    ]
    .into_iter()
    .map(|text| clean(text, max))
    .find(|text| !text.is_empty())
    .unwrap_or_default()
}

/// Where each node's subtree ends: the index of the first node after it.
fn subtree_ends<H>(raw: &[RawNode<H>]) -> Vec<usize> {
    let mut ends = vec![raw.len(); raw.len()];
    let mut open: Vec<usize> = Vec::new();
    for (index, node) in raw.iter().enumerate() {
        while let Some(&top) = open.last() {
            if raw[top].depth < node.depth {
                break;
            }
            ends[top] = index;
            open.pop();
        }
        open.push(index);
    }
    ends
}

/// [`build_elements`], also leaving out web content outside `viewport` (the
/// window, in screen points) and reading web pages more compactly: text
/// that only repeats its link's or button's label is dropped, runs of text
/// become one line, and unlabelled rows and cells are labelled by their text.
pub fn build<H>(raw: Vec<RawNode<H>>, max: usize, viewport: Option<Rect>) -> Built<H> {
    let ends = subtree_ends(&raw);
    let count = raw.len();
    let mut nodes: Vec<Option<RawNode<H>>> = raw.into_iter().map(Some).collect();
    let mut built = Built {
        elements: Vec::new(),
        truncated: false,
        offscreen: 0,
    };
    // Shown elements above the current node: depth, role and full label.
    let mut shown: Vec<(usize, String, String)> = Vec::new();
    let mut pages = Pages {
        window: viewport,
        open: Vec::new(),
    };
    let mut index = 0;
    while index < count {
        let Some(node) = nodes[index].take() else {
            index += 1;
            continue;
        };
        let depth = node.depth;
        while shown.last().is_some_and(|(d, _, _)| *d >= depth) {
            shown.pop();
        }
        pages.enter(depth);
        if !node.focused && pages.hidden(node.frame) {
            built.offscreen += 1;
            index = ends[index];
            continue;
        }
        pages.open(depth, &node.role, node.frame);
        let mut next = index + 1;
        let (role, mut label, value, flags_from) = if plain_text(&node) {
            let mut text = spoken(&node, 2000);
            let repeated = shown
                .last()
                .is_some_and(|(_, role, label)| repeats(role, label, &text));
            if text.is_empty() || repeated {
                index = ends[index];
                continue;
            }
            // Runs of sibling text read as one line.
            next = ends[index];
            while next < count && text.chars().count() < 300 {
                let Some(sibling) = nodes[next].as_ref() else {
                    break;
                };
                if sibling.depth != depth || !plain_text(sibling) {
                    break;
                }
                let more = spoken(sibling, 2000);
                if !more.is_empty() {
                    text.push(' ');
                    text.push_str(&more);
                }
                next = ends[next];
            }
            (node.role.clone(), clean(&text, 300), None, false)
        } else {
            let name = clean(&node.name, 80);
            let description = clean(&node.description, 80);
            let placeholder = clean(&node.placeholder, 60);
            let value = node
                .value
                .as_deref()
                .map(|value| clean(value, 120))
                .filter(|value| !value.is_empty())
                // A heading's value is its level, shown as a flag.
                .filter(|value| node.role != "heading" || value.parse::<u8>().is_err());
            let label = [&name, &description, &placeholder]
                .into_iter()
                .find(|text| !text.is_empty())
                .cloned()
                .unwrap_or_default();
            (node.role.clone(), label, value, true)
        };
        if flags_from && label.is_empty() && value.is_none() && named_by_contents(&role) {
            label = contents_label(&nodes[index + 1..ends[index]]);
        }
        let worth = !flags_from
            || !label.is_empty()
            || value.is_some()
            || node.actions.iter().any(|action| !incidental(action))
            || interactive(&role)
            || node.focused;
        if !worth {
            index = next;
            continue;
        }
        if built.elements.len() >= max {
            built.truncated = true;
            break;
        }
        let element_index = built.elements.len();
        let indent = "  ".repeat(shown.len());
        let mut line = format!("{indent}[{element_index}] {role}");
        if !label.is_empty() {
            line.push_str(&format!(" \"{label}\""));
        }
        if node.secure {
            line.push_str(" = [hidden]");
        } else if let Some(value) = value.as_ref().filter(|value| **value != label) {
            line.push_str(&format!(" = \"{value}\""));
        }
        if flags_from {
            let mut flags = Vec::new();
            let level = node
                .value
                .as_deref()
                .filter(|_| role == "heading")
                .and_then(|value| value.trim().parse::<u8>().ok())
                .map(|level| format!("level {level}"));
            if let Some(level) = &level {
                flags.push(level.as_str());
            }
            if !node.enabled {
                flags.push("disabled");
            }
            if node.focused {
                flags.push("focused");
            }
            if node.selected {
                flags.push("selected");
            }
            match node.checked {
                Some(true) => flags.push("checked"),
                Some(false) => flags.push("unchecked"),
                None => {}
            }
            if !flags.is_empty() {
                line.push_str(&format!(" ({})", flags.join(", ")));
            }
            // Scrolling into view, and on a page the context menu, are on
            // every element: they stay usable but aren't listed.
            let listed: Vec<&String> = node
                .actions
                .iter()
                .filter(|action| {
                    *action != "AXScrollToVisible" && !(pages.inside() && *action == "AXShowMenu")
                })
                .collect();
            if !listed.is_empty() {
                line.push_str(&format!(
                    " [actions: {}]",
                    serde_json::to_string(&listed).unwrap_or_default()
                ));
            }
        }
        let full = if flags_from {
            [&node.name, &node.description, &label]
                .into_iter()
                .map(|text| clean(text, 2000))
                .find(|text| !text.is_empty())
                .unwrap_or_default()
        } else {
            label.clone()
        };
        shown.push((depth, role.clone(), full));
        let short = if label.is_empty() {
            role.clone()
        } else {
            format!("{role} “{}”", clean(&label, 80))
        };
        built.elements.push(Element {
            index: element_index,
            line,
            label: short,
            frame: node.frame,
            secure: node.secure,
            focused: node.focused,
            actions: node.actions,
            handle: node.handle,
        });
        index = next;
    }
    built
}

/// The text inside an unlabelled row, cell or link, part by part.
fn contents_label<H>(inside: &[Option<RawNode<H>>]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut length = 0;
    for node in inside.iter().flatten() {
        if !plain_text(node) {
            continue;
        }
        let text = spoken(node, 120);
        if text.is_empty() || parts.last() == Some(&text) {
            continue;
        }
        length += text.chars().count() + 3;
        parts.push(text);
        if length > 160 {
            break;
        }
    }
    clean(&parts.join(" | "), 160)
}

/// While a platform reads a window: web content outside what its page
/// shows isn't read further, as a snapshot leaves it out anyway.
pub struct Reach {
    pages: Pages,
}

impl Reach {
    pub fn new(window: Rect) -> Self {
        Self {
            pages: Pages {
                window: Some(window),
                open: Vec::new(),
            },
        }
    }

    /// Whether to read the children of a node just read, in tree order.
    pub fn descend(&mut self, depth: usize, role: &str, frame: Option<Rect>) -> bool {
        self.pages.enter(depth);
        if self.pages.hidden(frame) {
            return false;
        }
        self.pages.open(depth, role, frame);
        true
    }
}

/// `AXStaticText` → `text`, `AXPopUpButton` → `pop-up button`.
pub fn humanize_role(role: &str) -> String {
    let bare = role.strip_prefix("AX").unwrap_or(role);
    match bare {
        "StaticText" => return "text".into(),
        "CheckBox" => return "checkbox".into(),
        "PopUpButton" => return "pop-up button".into(),
        "" => return "element".into(),
        _ => {}
    }
    let mut words = String::new();
    for (i, c) in bare.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            words.push(' ');
        }
        words.extend(c.to_lowercase());
    }
    words.replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(depth: usize, role: &str, name: &str, has_children: bool) -> RawNode<()> {
        RawNode {
            depth,
            role: role.into(),
            name: name.into(),
            value: None,
            description: String::new(),
            placeholder: String::new(),
            enabled: true,
            focused: false,
            selected: false,
            checked: None,
            secure: false,
            frame: None,
            actions: Vec::new(),
            has_children,
            handle: (),
        }
    }

    #[test]
    fn unlabelled_containers_fold_and_values_stay_secret() {
        let mut password = node(3, "text field", "Password", false);
        password.secure = true;
        password.value = Some("hunter2".into());
        let mut email = node(3, "text field", "Email", false);
        email.value = Some("ada@example.com".into());
        email.focused = true;
        let raw = vec![
            node(0, "window", "Sign in", true),
            node(1, "group", "", true),
            node(2, "group", "", true),
            email,
            password,
            node(3, "button", "Sign in", false),
            node(3, "image", "", false),
        ];
        let (elements, truncated) = build_elements(raw, 100);
        assert!(!truncated);
        let lines: Vec<&str> = elements.iter().map(|e| e.line.as_str()).collect();
        assert_eq!(
            lines,
            [
                "[0] window \"Sign in\"",
                "  [1] text field \"Email\" = \"ada@example.com\" (focused)",
                "  [2] text field \"Password\" = [hidden]",
                "  [3] button \"Sign in\"",
            ]
        );
        assert!(!lines.iter().any(|line| line.contains("hunter2")));
        assert_eq!(elements[3].label, "button “Sign in”");
        let (cut, truncated) = build_elements(
            vec![node(0, "button", "A", false), node(0, "button", "B", false)],
            1,
        );
        assert!(truncated && cut.len() == 1);
    }

    fn at(mut node: RawNode<()>, y: f64) -> RawNode<()> {
        node.frame = Some(Rect {
            x: 10.0,
            y,
            width: 300.0,
            height: 20.0,
        });
        node
    }

    fn acting(mut node: RawNode<()>, actions: &[&str]) -> RawNode<()> {
        node.actions = actions.iter().map(|a| a.to_string()).collect();
        node
    }

    #[test]
    fn web_pages_read_compactly_and_leave_out_what_is_off_screen() {
        let menu = ["AXShowMenu", "AXScrollToVisible"];
        let window = Rect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
        };
        let raw = vec![
            node(0, "window", "Docs — Browser", true),
            node(1, "web area", "Docs", true),
            {
                let mut heading = at(node(2, "heading", "", true), 100.0);
                heading.value = Some("2".into());
                heading
            },
            acting(at(node(3, "text", "Getting started", false), 100.0), &menu),
            at(node(2, "group", "", true), 140.0),
            acting(at(node(3, "text", "Read the ", false), 140.0), &menu),
            acting(
                at(node(3, "link", "", true), 140.0),
                &["AXPress", "AXShowMenu"],
            ),
            acting(at(node(4, "text", "docs", false), 140.0), &menu),
            acting(at(node(3, "text", " for more.", false), 140.0), &menu),
            acting(
                at(node(3, "text", " Second  sentence.", false), 140.0),
                &menu,
            ),
            at(node(2, "row", "", true), 200.0),
            at(node(3, "cell", "", true), 200.0),
            at(node(4, "text", "Inbox", false), 200.0),
            at(node(3, "cell", "", true), 200.0),
            at(node(4, "text", "3 unread", false), 200.0),
            // Labelled cells, as browsers expose table cells.
            at(node(2, "group", "Unread", true), 230.0),
            at(node(3, "text", "Unread", false), 230.0),
            // Scrolled away, squashed onto the page's edge.
            {
                let mut sliver = node(2, "button", "Sliver", false);
                sliver.frame = Some(Rect {
                    x: 10.0,
                    y: 800.0,
                    width: 80.0,
                    height: 0.0,
                });
                sliver
            },
            at(node(2, "group", "", true), 2400.0),
            at(node(3, "button", "Far away", false), 2400.0),
            at(node(3, "text", "Far text", false), 2420.0),
            at(node(2, "button", "Back to top", false), 700.0),
        ];
        let built = build(raw, 100, Some(window));
        let lines: Vec<&str> = built.elements.iter().map(|e| e.line.as_str()).collect();
        assert_eq!(
            lines,
            [
                "[0] window \"Docs — Browser\"",
                "  [1] web area \"Docs\"",
                "    [2] heading \"Getting started\" (level 2)",
                "    [3] text \"Read the\"",
                "    [4] link \"docs\" [actions: [\"AXPress\"]]",
                "    [5] text \"for more. Second sentence.\"",
                "    [6] row \"Inbox | 3 unread\"",
                "      [7] cell \"Inbox\"",
                "      [8] cell \"3 unread\"",
                "    [9] group \"Unread\"",
                "    [10] button \"Back to top\"",
            ]
        );
        assert_eq!(built.offscreen, 2);
        assert_eq!(built.elements[4].label, "link “docs”");
        // Text keeps its actions for the agent to use, just not on its line.
        assert_eq!(built.elements[3].actions, menu);
        // Without a window to measure against, nothing is left out.
        let raw = vec![
            node(0, "web area", "Docs", true),
            at(node(1, "button", "Far away", false), 2400.0),
        ];
        assert_eq!(build(raw, 100, None).elements.len(), 2);
    }

    #[test]
    fn reading_stops_at_web_content_outside_the_window() {
        let window = Rect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
        };
        let far = Some(Rect {
            x: 0.0,
            y: 5000.0,
            width: 100.0,
            height: 100.0,
        });
        let near = Some(Rect {
            x: 0.0,
            y: 100.0,
            width: 100.0,
            height: 100.0,
        });
        let mut reach = Reach::new(window);
        assert!(reach.descend(0, "window", None));
        // A native list scrolled past the window is still read in full.
        assert!(reach.descend(1, "list", far));
        assert!(reach.descend(1, "web area", near));
        assert!(reach.descend(2, "group", near));
        assert!(!reach.descend(2, "group", far));
        let sliver = Some(Rect {
            x: 0.0,
            y: 800.0,
            width: 100.0,
            height: 0.0,
        });
        assert!(!reach.descend(2, "group", sliver));
        assert!(reach.descend(3, "group", None));
        // Leaving the page ends the rule.
        assert!(reach.descend(1, "toolbar", far));
    }

    #[test]
    fn apps_resolve_by_id_name_or_pid_and_secrets_are_blocked() {
        let app = |name: &str, id: &str, pid| AppInfo {
            name: name.into(),
            id: id.into(),
            pid,
            active: false,
        };
        let apps = vec![
            app("Safari", "com.apple.Safari", 1),
            app(
                "Safari Technology Preview",
                "com.apple.SafariTechnologyPreview",
                2,
            ),
            app("TextEdit", "com.apple.TextEdit", 3),
        ];
        assert_eq!(resolve_app(&apps, "safari").unwrap().pid, 1);
        assert_eq!(resolve_app(&apps, "com.apple.TextEdit").unwrap().pid, 3);
        assert_eq!(resolve_app(&apps, "pid:2").unwrap().pid, 2);
        assert_eq!(resolve_app(&apps, "textedit").unwrap().pid, 3);
        assert!(resolve_app(&apps, "Technology").is_ok());
        assert!(resolve_app(&apps, "com.apple").is_err());
        assert!(resolve_app(&apps, "Notes").is_err());
        assert!(is_blocked(&app(
            "1Password 8",
            "com.1password.1password",
            9
        )));
        assert!(is_blocked(&app("Passwords", "com.apple.Passwords", 9)));
        assert!(!is_blocked(&app("Safari", "com.apple.Safari", 1)));
        assert_eq!(humanize_role("AXStaticText"), "text");
        assert_eq!(humanize_role("AXTextField"), "text field");
        assert_eq!(humanize_role("AXRadioButton"), "radio button");
    }
}

fn serialize_window_index<S: serde::Serializer>(
    index: &usize,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64((*index + 1) as u64)
}
