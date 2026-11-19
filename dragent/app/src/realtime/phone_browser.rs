//! A browser tab, driven from a phone.
//!
//! The phone sees the page as pictures and sends back what the person does:
//! an address to open, back, forward, reload, a tap at a point, a scroll,
//! text and keys. Each of those is carried out by the same control layer
//! agents use (`dragent browser`), on the window's UI thread.
//!
//! The phone never sends script: every script that runs in the page is one
//! of the fixed ones built here, with the phone's values passed as data.

use serde_json::{json, Value};

/// Widest picture sent to a phone, in pixels. Wider pages are scaled down.
pub const MAX_WIDTH: u32 = 1000;
/// Tallest picture sent, in pixels.
pub const MAX_HEIGHT: u32 = 2400;
/// Largest picture, as JPEG bytes. Above it the quality is lowered.
pub const MAX_IMAGE_BYTES: usize = 520 * 1024;
/// Longest address or text a phone may send.
pub const MAX_TEXT_BYTES: usize = 8 * 1024;

/// Keys a phone may press in the page.
const KEYS: &[&str] = &[
    "Enter", "Tab", "Backspace", "Delete", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
    "Home", "End", "PageUp", "PageDown", "Space",
];

/// What the window's browser control is asked to run: a tool and its arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub tool: &'static str,
    pub arguments: Value,
}

/// What the phone gets back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// `{done: true}`.
    Done,
    /// The page's address, title and scroll position.
    Info,
    /// A picture of the page.
    Picture,
}

/// The desktop's own page scripts run inline, not through the page's
/// `eval`: pages whose CSP forbids eval (AWS, Google and bank sign-ins)
/// would otherwise refuse every step of the phone's view.
fn eval(script: String) -> Step {
    Step { tool: "eval", arguments: json!({"script": script, "inline": true}) }
}

/// Page script shared by the scripts below: `pinned()` lists the elements
/// that stay put while the page scrolls (fixed, or sticky and stuck), with
/// where they are on the screen. A real browser keeps these in place; a
/// picture of the screen has them baked in, so they are captured apart.
const PINNED_JS: &str = "const pinned = () => { const out = []; const H = innerHeight, W = innerWidth; \
    for (const el of document.querySelectorAll('body *')) { \
      const s = getComputedStyle(el); const p = s.position; \
      if (p !== 'fixed' && p !== 'sticky') continue; \
      if (s.visibility === 'hidden' || s.display === 'none' || parseFloat(s.opacity) === 0) continue; \
      const r = el.getBoundingClientRect(); \
      if (r.width < 8 || r.height < 4 || r.bottom <= 0 || r.top >= H || r.right <= 0 || r.left >= W) continue; \
      if (r.height > H * 0.5) continue; \
      if (p === 'sticky') { const top = parseFloat(s.top), bottom = parseFloat(s.bottom); \
        const atTop = !isNaN(top) && Math.abs(r.top - top) < 1.5; \
        const atBottom = !isNaN(bottom) && Math.abs(H - r.bottom - bottom) < 1.5; \
        if (!atTop && !atBottom) continue; } \
      out.push([el, r]); if (out.length >= 12) break; } \
    return out.filter(([el]) => !out.some(([other]) => other !== el && other.contains(el))).slice(0, 6); }; \
    const hidePinned = () => { for (const [el] of pinned()) { \
      el.setAttribute('data-dragent-pin', el.style.getPropertyValue('visibility') + '|' + el.style.getPropertyPriority('visibility')); \
      el.style.setProperty('visibility', 'hidden', 'important'); } }; \
    const showPinned = () => { for (const el of document.querySelectorAll('[data-dragent-pin]')) { \
      const [value, priority] = el.getAttribute('data-dragent-pin').split('|'); \
      el.removeAttribute('data-dragent-pin'); \
      if (value) el.style.setProperty('visibility', value, priority); else el.style.removeProperty('visibility'); } }; \
    const pageInfo = () => { let bg = getComputedStyle(document.body).backgroundColor; \
      if (!bg || bg === 'transparent' || bg.endsWith(', 0)')) bg = getComputedStyle(document.documentElement).backgroundColor; \
      return {url: location.href, title: document.title, width: innerWidth, height: innerHeight, \
        top: Math.round(scrollY), full: Math.round(document.documentElement.scrollHeight), bg}; }; ";

/// The page's address, size and position, and where its pinned elements are.
fn info_script() -> String {
    format!(
        "(() => {{ {PINNED_JS} showPinned(); const info = pageInfo(); \
         info.pinned = pinned().map(([, r]) => [r.left, r.top, r.width, r.height]); \
         return JSON.stringify(info); }})()"
    )
}

/// Hide the pinned elements for a picture of the page alone.
pub fn hide_pinned_script() -> Step {
    eval(format!("(() => {{ {PINNED_JS} hidePinned(); return 'ok'; }})()"))
}

/// Put the pinned elements back.
pub fn show_pinned_script() -> Step {
    eval(format!("(() => {{ {PINNED_JS} showPinned(); return 'ok'; }})()"))
}

fn unit(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite() && (0.0..=1.0).contains(n))
}

/// A scroll position a phone may name: a pixel offset from the page's top.
fn page_top(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite() && (0.0..=10_000_000.0).contains(n)).map(f64::round)
}

fn text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|t| !t.is_empty() && t.len() <= MAX_TEXT_BYTES && !t.contains('\0'))
}

/// The step for a phone's action and what it answers with. `Err(())` for a
/// request that isn't one of the actions below or has bad values.
pub fn plan(action: &str, params: &Value, picture_path: &str) -> Result<(Step, Answer), ()> {
    let done = |step: Step| Ok((step, Answer::Done));
    match action {
        "open" => {
            let url = text(params.get("url")).ok_or(())?;
            done(Step { tool: "open", arguments: json!({"url": url}) })
        }
        "back" => done(Step { tool: "back", arguments: json!({}) }),
        "forward" => done(Step { tool: "forward", arguments: json!({}) }),
        "reload" => done(Step { tool: "reload", arguments: json!({}) }),
        "info" => Ok((eval(info_script()), Answer::Info)),
        "picture" => Ok((
            Step { tool: "picture", arguments: json!({"path": picture_path}) },
            Answer::Picture,
        )),
        // A point of the visible page, as fractions of its width and height.
        "tap" => {
            let x = unit(params.get("x")).ok_or(())?;
            let y = unit(params.get("y")).ok_or(())?;
            // The click runs a moment after the script answers: a click that
            // leaves the page would otherwise take the answer with it, and
            // the phone would wait for one that never comes.
            done(eval(format!(
                "(() => {{ const x = {x} * innerWidth, y = {y} * innerHeight; \
                 const el = document.elementFromPoint(x, y); if (!el) return 'nothing'; \
                 setTimeout(() => {{ \
                   const at = {{bubbles: true, cancelable: true, view: window, clientX: x, clientY: y, button: 0}}; \
                   for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup']) \
                     el.dispatchEvent(new (type.startsWith('pointer') ? PointerEvent : MouseEvent)(type, at)); \
                   if (typeof el.focus === 'function') el.focus(); el.click(); }}, 0); \
                 return el.tagName; }})()"
            )))
        }
        // Screens to scroll by: 1 is one screen down, -1 one up. With a
        // point (where the finger is), whatever scrolls under that point is
        // scrolled: many pages scroll a panel inside them, not the window.
        "scroll" => {
            let by = params.get("by").and_then(Value::as_f64).filter(|n| n.is_finite() && n.abs() <= 20.0).ok_or(())?;
            let point = match (params.get("x"), params.get("y")) {
                (None, None) => None,
                (x, y) => Some((unit(x).ok_or(())?, unit(y).ok_or(())?)),
            };
            let (x, y) = point.unwrap_or((0.5, 0.5));
            done(eval(format!(
                "(() => {{ const dy = {by} * innerHeight; \
                 let el = document.elementFromPoint({x} * innerWidth, {y} * innerHeight); \
                 while (el && el !== document.body && el !== document.documentElement) {{ \
                   const style = getComputedStyle(el); \
                   const scrolls = (style.overflowY === 'auto' || style.overflowY === 'scroll' || style.overflowY === 'overlay') \
                     && el.scrollHeight > el.clientHeight + 1; \
                   const room = dy > 0 ? el.scrollTop + el.clientHeight < el.scrollHeight - 1 : el.scrollTop > 0; \
                   if (scrolls && room) {{ el.scrollBy({{top: dy, behavior: 'instant'}}); return 'panel'; }} \
                   el = el.parentElement; }} \
                 window.scrollBy({{top: dy, behavior: 'instant'}}); return 'window'; }})()"
            )))
        }
        // The window's own scroll position, in the page's pixels from its top.
        "scroll_to" => {
            let top = page_top(params.get("top")).ok_or(())?;
            done(eval(format!(
                "(() => {{ window.scrollTo({{top: {top}, behavior: 'instant'}}); return 'ok'; }})()"
            )))
        }
        "type" => {
            let value = serde_json::to_string(text(params.get("text")).ok_or(())?).map_err(|_| ())?;
            done(eval(format!(
                "(() => {{ const el = document.activeElement; const text = {value}; \
                 if (!el || el === document.body) return 'nothing'; \
                 if (document.execCommand && document.execCommand('insertText', false, text)) return 'ok'; \
                 if ('value' in el) {{ el.value += text; el.dispatchEvent(new Event('input', {{bubbles: true}})); return 'ok'; }} \
                 return 'nothing'; }})()"
            )))
        }
        "press" => {
            let key = params.get("key").and_then(Value::as_str).filter(|k| KEYS.contains(k)).ok_or(())?;
            done(Step { tool: "press", arguments: json!({"key": key}) })
        }
        _ => Err(()),
    }
}

/// What a phone's request does before the page is shown back to it.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Only look at the page.
    View,
    /// The phone left the tab.
    Release,
    /// Do this first, wait `settle` for the page to react, then look.
    Act { step: Step, settle: std::time::Duration },
    /// Look at another part of the page (the screen ahead of where the
    /// person is scrolling) and put the page back where it was.
    Peek { top: f64 },
}

/// Scroll to `top` for a peek and hide the pinned elements there, answering
/// with where the page was and what it is now.
pub fn peek_script(top: f64) -> Step {
    eval(format!(
        "(() => {{ {PINNED_JS} showPinned(); const was = Math.round(scrollY); \
         window.scrollTo({{top: {top}, behavior: 'instant'}}); hidePinned(); \
         return JSON.stringify(Object.assign({{was}}, pageInfo())); }})()"
    ))
}

/// Put the page back after a peek.
pub fn restore_script(top: f64) -> Step {
    eval(format!(
        "(() => {{ {PINNED_JS} showPinned(); window.scrollTo({{top: {top}, behavior: 'instant'}}); return 'ok'; }})()"
    ))
}

/// Where the page was before a peek, from the peek script's answer.
pub fn peek_was(raw: &str) -> Option<f64> {
    serde_json::from_str::<Value>(raw).ok()?.get("was")?.as_f64().filter(|n| n.is_finite() && *n >= 0.0)
}

/// A phone's `browser.act` request, checked.
pub fn request(action: &str, params: &Value) -> Result<Request, ()> {
    use std::time::Duration;
    match action {
        "view" => Ok(Request::View),
        "release" => Ok(Request::Release),
        "peek" => Ok(Request::Peek { top: page_top(params.get("top")).ok_or(())? }),
        // These are steps of looking, never asked for by name.
        "info" | "picture" => Err(()),
        _ => {
            let (step, _) = plan(action, params, "")?;
            let settle = Duration::from_millis(match action {
                // A click or a key often changes the page a moment later.
                "tap" | "press" => 260,
                "type" => 60,
                "scroll" | "scroll_to" => 25,
                // These answer once the new page has loaded.
                _ => 120,
            });
            Ok(Request::Act { step, settle })
        }
    }
}

/// The steps that look at the page: its address and position, then a picture.
pub fn look(picture_path: &str) -> (Step, Step) {
    (
        eval(info_script()),
        Step { tool: "picture", arguments: json!({"path": picture_path}) },
    )
}

/// A phone's mobile-view size, in points: `{width, height}` within what a
/// phone or small tablet screen can be.
pub fn viewport(value: &Value) -> Option<(f32, f32)> {
    let side = |key: &str, least: f64, most: f64| {
        value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite() && (least..=most).contains(n))
    };
    if value.as_object()?.keys().any(|k| k != "width" && k != "height") {
        return None;
    }
    Some((side("width", 240.0, 1024.0)?.round() as f32, side("height", 240.0, 1600.0)?.round() as f32))
}

/// What the window answers when the desktop has taken the tab back.
pub const TAKEN_BACK: &str = "desktop_in_control";

/// The page's own answer to the info script, checked and bounded.
pub fn info(raw: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let number = |key: &str| value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.0);
    let clip = |key: &str, limit: usize| -> String {
        value.get(key).and_then(Value::as_str).unwrap_or_default().chars().take(limit).collect()
    };
    // The page's background, as the browser reports it ("rgb(…)"), so the
    // phone can paint what it hasn't been sent yet in the page's own colour.
    let bg = value
        .get("bg")
        .and_then(Value::as_str)
        .filter(|c| c.len() <= 40 && c.starts_with("rgb") && c.chars().all(|ch| ch.is_ascii_alphanumeric() || " (),.".contains(ch)));
    Some(json!({"page": true, "url": clip("url", 4096), "title": clip("title", 300),
        "width": number("width")?, "height": number("height")?, "top": number("top")?, "full": number("full")?,
        "bg": bg}))
}

/// Where the page's pinned elements are on the screen, from the info
/// script's answer: `(left, top, width, height)` in the page's pixels.
pub fn pinned(raw: &str) -> Vec<(f64, f64, f64, f64)> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else { return Vec::new() };
    let Some(list) = value.get("pinned").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .take(6)
        .filter_map(|rect| {
            let n = |i: usize| rect.get(i).and_then(Value::as_f64).filter(|n| n.is_finite() && n.abs() < 100_000.0);
            Some((n(0)?, n(1)?, n(2)?, n(3)?))
        })
        .filter(|(_, _, w, h)| *w >= 1.0 && *h >= 1.0)
        .collect()
}

/// Largest picture of one pinned element, as JPEG bytes.
const MAX_OVERLAY_BYTES: usize = 160 * 1024;

/// The pinned elements cut out of a picture of the whole screen, each with
/// its place on the screen (page pixels), for the phone to keep in place
/// over the scrolling page.
pub fn overlays(png: &[u8], pinned: &[(f64, f64, f64, f64)], page_width: f64) -> Vec<Value> {
    use base64::Engine as _;
    let Ok(image) = image::load_from_memory_with_format(png, image::ImageFormat::Png) else { return Vec::new() };
    if page_width < 1.0 {
        return Vec::new();
    }
    let k = image.width() as f64 / page_width;
    let mut out = Vec::new();
    for (left, top, width, height) in pinned {
        // The part of the element that is on the screen.
        let x0 = (left.max(0.0) * k).round() as u32;
        let y0 = (top.max(0.0) * k).round() as u32;
        let x1 = (((left + width) * k).round() as u32).min(image.width());
        let y1 = (((top + height) * k).round() as u32).min(image.height());
        if x1 <= x0 + 1 || y1 <= y0 + 1 {
            continue;
        }
        let mut cut = image.crop_imm(x0, y0, x1 - x0, y1 - y0);
        if cut.width() > MAX_WIDTH {
            cut = cut.resize(MAX_WIDTH, MAX_HEIGHT, image::imageops::FilterType::Triangle);
        }
        let rgb = cut.to_rgb8();
        let mut bytes = Vec::new();
        if image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80)
            .encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
            .is_err()
            || bytes.len() > MAX_OVERLAY_BYTES
        {
            continue;
        }
        out.push(json!({"x": x0 as f64 / k, "y": y0 as f64 / k, "width": (x1 - x0) as f64 / k,
            "height": (y1 - y0) as f64 / k, "image": base64::engine::general_purpose::STANDARD.encode(&bytes)}));
    }
    out
}

/// A PNG of the page as a JPEG a phone can take: scaled to fit
/// [`MAX_WIDTH`] × [`MAX_HEIGHT`] and kept under [`MAX_IMAGE_BYTES`].
pub fn picture(png: &[u8]) -> Result<Value, &'static str> {
    use base64::Engine as _;
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(|_| "unavailable")?;
    let image = if image.width() > MAX_WIDTH || image.height() > MAX_HEIGHT {
        image.resize(MAX_WIDTH, MAX_HEIGHT, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let rgb = image.to_rgb8();
    let (width, height) = (rgb.width(), rgb.height());
    if width == 0 || height == 0 {
        return Err("unavailable");
    }
    let mut bytes = Vec::new();
    for quality in [72u8, 55, 38, 24] {
        bytes.clear();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
            .map_err(|_| "unavailable")?;
        if bytes.len() <= MAX_IMAGE_BYTES {
            break;
        }
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err("unavailable");
    }
    Ok(json!({"image": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "format": "jpeg", "width": width, "height": height}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_names_actions_and_never_sends_script() {
        let (step, answer) = plan("open", &json!({"url": "localhost:3000"}), "/p").unwrap();
        assert_eq!((step.tool, answer), ("open", Answer::Done));
        assert_eq!(step.arguments["url"], "localhost:3000");
        let (step, answer) = plan("picture", &json!({}), "/tmp/p.png").unwrap();
        assert_eq!((step.tool, answer), ("picture", Answer::Picture));
        assert_eq!(step.arguments["path"], "/tmp/p.png");
        assert_eq!(plan("info", &json!({}), "/p").unwrap().1, Answer::Info);
        // A scroll under the finger scrolls the panel there, else the window.
        let (step, _) = plan("scroll", &json!({"by": 0.5, "x": 0.25, "y": 0.75}), "/p").unwrap();
        let script = step.arguments["script"].as_str().unwrap();
        assert!(script.contains("elementFromPoint(0.25 * innerWidth, 0.75 * innerHeight)"), "{script}");
        assert!(script.contains("window.scrollBy"));
        assert_eq!(plan("press", &json!({"key": "Enter"}), "/p").unwrap().0.arguments["key"], "Enter");
        // Text is passed to the page as data, whatever it contains.
        let (step, _) = plan("type", &json!({"text": "\"); alert(1); (\""}), "/p").unwrap();
        let script = step.arguments["script"].as_str().unwrap();
        assert!(script.contains(r#"const text = "\"); alert(1); (\"";"#), "{script}");
        for (action, params) in [
            ("eval", json!({"script": "alert(1)"})),
            ("tap", json!({"x": 1.5, "y": 0.2})),
            ("tap", json!({"x": "0.5", "y": 0.2})),
            ("scroll", json!({"by": 1e9})),
            ("scroll", json!({"by": 1, "x": 0.5})),
            ("scroll", json!({"by": 1, "x": 0.5, "y": 3})),
            ("press", json!({"key": "F12"})),
            ("type", json!({"text": ""})),
            ("open", json!({})),
        ] {
            assert!(plan(action, &params, "/p").is_err(), "{action} {params}");
        }
    }

    #[test]
    fn a_request_is_an_action_then_a_look() {
        assert_eq!(request("view", &json!({})), Ok(Request::View));
        assert_eq!(request("release", &json!({})), Ok(Request::Release));
        let Ok(Request::Act { step, settle }) = request("scroll", &json!({"by": 0.5})) else { panic!() };
        assert_eq!(step.tool, "eval");
        assert!(settle < std::time::Duration::from_millis(100), "scrolling shows at once");
        assert!(request("picture", &json!({})).is_err());
        assert_eq!(request("peek", &json!({"top": 812.4})), Ok(Request::Peek { top: 812.0 }));
        assert!(request("peek", &json!({"top": -5})).is_err());
        let Ok(Request::Act { step, .. }) = request("scroll_to", &json!({"top": 1200})) else { panic!() };
        assert!(step.arguments["script"].as_str().unwrap().contains("window.scrollTo({top: 1200"));
        assert!(request("scroll_to", &json!({"top": "1200"})).is_err());
        assert_eq!(peek_was(r#"{"was":640,"top":1452}"#), Some(640.0));
        assert!(peek_script(1452.0).arguments["script"].as_str().unwrap().contains("const was"));
        assert!(request("eval", &json!({"script": "x"})).is_err());
        let (info, picture) = look("/tmp/p.png");
        assert_eq!((info.tool, picture.tool), ("eval", "picture"));
        // The view's scripts run inline, so sign-in pages that forbid eval work.
        assert_eq!(info.arguments["inline"], json!(true));
        assert_eq!(step.arguments["inline"], json!(true));
    }

    #[test]
    fn a_mobile_view_is_a_phone_sized_page() {
        assert_eq!(viewport(&json!({"width": 390.4, "height": 664})), Some((390.0, 664.0)));
        for bad in [json!({"width": 20, "height": 600}), json!({"width": 390}), json!({"width": 390, "height": 9000}),
            json!({"width": 390, "height": 600, "scale": 3}), json!("390x600")] {
            assert_eq!(viewport(&bad), None, "{bad}");
        }
    }

    #[test]
    fn page_info_is_checked_before_it_reaches_the_phone() {
        let v = info(r#"{"url":"http://localhost:3000/","title":"App","width":1200,"height":800,"top":40,"full":2600}"#).unwrap();
        assert_eq!(v["url"], "http://localhost:3000/");
        assert_eq!(v["full"], 2600.0);
        assert!(info("not json").is_none());
        assert!(info(r#"{"url":"x","title":"y","width":-1,"height":1,"top":0,"full":1}"#).is_none());
    }

    #[test]
    fn pinned_elements_are_cut_out_with_their_place_on_the_screen() {
        // A 400x300 page at 2x: a 60px header across the top, then content.
        let shot = image::RgbImage::from_fn(800, 600, |_, y| if y < 120 { image::Rgb([200, 30, 30]) } else { image::Rgb([20, 20, 20]) });
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(shot)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let raw = r#"{"url":"x","title":"y","width":400,"height":300,"top":900,"full":4000,"bg":"rgb(5, 7, 9)",
            "pinned":[[0,0,400,60],[0,-500,400,20],["bad"]]}"#;
        let rects = pinned(raw);
        assert_eq!(rects, vec![(0.0, 0.0, 400.0, 60.0), (0.0, -500.0, 400.0, 20.0)]);
        let cut = overlays(&png, &rects, 400.0);
        assert_eq!(cut.len(), 1, "an element off the screen has no picture");
        assert_eq!((cut[0]["x"].as_f64(), cut[0]["y"].as_f64(), cut[0]["width"].as_f64(), cut[0]["height"].as_f64()),
            (Some(0.0), Some(0.0), Some(400.0), Some(60.0)));
        use base64::Engine as _;
        let jpeg = base64::engine::general_purpose::STANDARD.decode(cut[0]["image"].as_str().unwrap()).unwrap();
        let header = image::load_from_memory(&jpeg).unwrap().to_rgb8();
        assert_eq!((header.width(), header.height()), (800, 120));
        assert!(header.get_pixel(400, 60)[0] > 150, "the header's own colour");
        assert_eq!(info(raw).unwrap()["bg"], "rgb(5, 7, 9)");
        assert!(info(r#"{"url":"x","title":"y","width":1,"height":1,"top":0,"full":1,"bg":"url(javascript:1)"}"#).unwrap()["bg"].is_null());
        // The scripts that hide and restore them carry the same detection.
        assert!(hide_pinned_script().arguments["script"].as_str().unwrap().contains("hidePinned()"));
        assert!(peek_script(10.0).arguments["script"].as_str().unwrap().contains("hidePinned()"));
        assert!(restore_script(10.0).arguments["script"].as_str().unwrap().contains("showPinned()"));
    }

    #[test]
    fn a_large_page_picture_is_scaled_to_what_a_phone_takes() {
        let big = image::RgbImage::from_fn(2400, 1600, |x, y| image::Rgb([(x % 255) as u8, (y % 255) as u8, 90]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(big)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let v = picture(&png).unwrap();
        assert_eq!(v["width"], MAX_WIDTH);
        assert_eq!(v["height"], 667);
        use base64::Engine as _;
        let jpeg = base64::engine::general_purpose::STANDARD.decode(v["image"].as_str().unwrap()).unwrap();
        assert!(jpeg.len() <= MAX_IMAGE_BYTES);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "a JPEG");
        assert!(picture(b"not a png").is_err());
    }
}
