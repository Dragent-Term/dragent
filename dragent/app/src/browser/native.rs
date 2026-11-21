//! The platform web engine behind one browser pane: WKWebView on macOS,
//! WebView2 on Windows and WebKitGTK on Linux, embedded as a child view of
//! the workspace window. Callbacks only record into [`PageState`] and wake the
//! owning view; they never re-enter the engine or the app.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use async_channel::Sender;
use dragent_workspace_model::{BrowserPaneSession, BrowserUrl};
use warpui::geometry::rect::RectF;

/// What the page reported since the view last looked.
#[derive(Default)]
pub(super) struct PageState {
    pub title: String,
    /// Last committed top-level address.
    pub url: Option<String>,
    pub loading: bool,
    /// A load started or finished since the pane last looked: the address
    /// answered, even when a fast page finished before the pane saw it
    /// loading.
    pub responded: bool,
    /// The user typed into the page; it is kept alive instead of being
    /// discarded to save memory.
    pub edited: bool,
    pub focused: bool,
    /// Address the page asked to open in a new window.
    pub new_window: Option<String>,
    /// A navigation the page attempted that the browser does not follow.
    pub blocked: Option<String>,
    pub downloaded: Option<Result<PathBuf, String>>,
    pub crashed: bool,
    /// A link the page wants another app to open; asked about first.
    pub app_link: Option<String>,
    /// Zoom steps from Cmd/Ctrl + scroll in the page (1 in, -1 out).
    pub zoom_steps: Vec<i8>,
    /// The page's icon address, as the page last declared it.
    pub favicon: Option<String>,
    pub design_messages: std::collections::VecDeque<String>,
    pub document_sequence: u64,
}

type Page = Rc<RefCell<PageState>>;

fn record(page: &Page, wake: &Sender<()>, update: impl FnOnce(&mut PageState)) {
    update(&mut page.borrow_mut());
    // Bounded(1): pending wakes coalesce into one refresh.
    let _ = wake.try_send(());
}

/// Reports focus and the first edit back to the pane. Main frame only.
const PAGE_SCRIPT: &str = r#"(() => {
  const post = (message) => { try { window.ipc.postMessage(message); } catch (_) {} };
  addEventListener('pointerdown', () => post('focus'), true);
  addEventListener('focusin', () => post('focus'), true);
  const edited = event => {
    if (event.composedPath().some(node => node.hasAttribute?.('data-dragent-design'))) return;
    post('edited'); removeEventListener('input', edited, true);
  };
  addEventListener('input', edited, true);
})();"#;

/// Cmd + scroll (Ctrl + scroll on Linux, where a pinch arrives as one too)
/// zooms the page as a desktop browser does, unless the page used the
/// gesture itself (maps, canvases). Windows' engine already zooms on
/// Ctrl + scroll and pinch by itself.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const ZOOM_SCRIPT: &str = r#"(() => {
  const key = navigator.platform.startsWith('Mac') ? 'metaKey' : 'ctrlKey';
  let travel = 0, last = 0;
  addEventListener('wheel', event => {
    if (!event[key] || event.defaultPrevented) return;
    event.preventDefault();
    const now = Date.now();
    if (now - last > 400) travel = 0;
    last = now;
    travel += event.deltaMode === 1 ? event.deltaY * 16 : event.deltaY;
    if (Math.abs(travel) < 50) return;
    try { window.ipc.postMessage(travel < 0 ? 'zoom:1' : 'zoom:-1'); } catch (_) {}
    travel = 0;
  }, { passive: false });
})();"#;

/// Reports the page's declared icon (main frame only), on load and when a
/// page changes it later.
const FAVICON_SCRIPT: &str = r#"(() => {
  if (window.top !== window) return;
  let sent = null;
  const size = link => Math.max(0, ...(link.getAttribute('sizes') || '').split(/\s+/).map(s => parseInt(s, 10) || 0));
  const svg = link => /svg/i.test(link.type) || /\.svg($|\?)/i.test(link.href);
  // An SVG scales; otherwise the smallest icon of at least 32 px draws best.
  const fit = link => svg(link) ? -1 : (size(link) >= 32 ? size(link) : 1000 + (32 - size(link)));
  const report = () => {
    const links = [...document.querySelectorAll('link[rel~="icon" i], link[rel="apple-touch-icon" i]')]
      .filter(link => /^https?:/.test(link.href))
      .sort((a, b) => fit(a) - fit(b));
    const href = links[0]?.href || (/^https?:/.test(location.href) ? new URL('/favicon.ico', location.href).href : null);
    if (href && href !== sent) {
      sent = href;
      try { window.ipc.postMessage('favicon:' + href); } catch (_) {}
    }
  };
  addEventListener('DOMContentLoaded', () => {
    report();
    // Pages that change their icon later (unread counts) change it in <head>.
    if (document.head) new MutationObserver(report).observe(document.head, { childList: true, subtree: true, attributes: true, attributeFilter: ['href', 'rel'] });
  });
})();"#;

#[cfg(not(target_family = "wasm"))]
pub(super) struct Engine {
    view: wry::WebView,
    // The context and parent window outlive the guest. UI-thread objects only.
    _context: wry::WebContext,
    _parent: Rc<dyn warpui::platform::Window>,
    /// Keeps WebKit's inspector out of the workspace window (macOS).
    #[cfg(target_os = "macos")]
    _inspector_anchor: Option<objc2::rc::Retained<objc2_app_kit::NSView>>,
    bounds: Option<RectF>,
    visible: bool,
    /// Where the page is parked out of sight, while it is.
    parked: Option<RectF>,
}

#[cfg(not(target_family = "wasm"))]
struct Parent(Rc<dyn warpui::platform::Window>);

#[cfg(not(target_family = "wasm"))]
impl raw_window_handle::HasWindowHandle for Parent {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = self
            .0
            .native_window_handle()
            .ok_or(raw_window_handle::HandleError::Unavailable)?;
        // The parent window is retained for the engine's whole lifetime, and
        // the guest is created and destroyed on the UI thread.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

#[cfg(not(target_family = "wasm"))]
impl Engine {
    #[cfg(target_os = "linux")]
    pub fn pump_events() {
        let start = std::time::Instant::now();
        for _ in 0..16 {
            if !gtk::events_pending() || start.elapsed() >= std::time::Duration::from_millis(2) { break; }
            gtk::main_iteration_do(false);
        }
    }

    pub fn create(
        parent: Rc<dyn warpui::platform::Window>,
        session: &BrowserPaneSession,
        bounds: RectF,
        page: Page,
        wake: Sender<()>,
        mobile: bool,
    ) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        {
            // Wry embeds into X11 windows only; never fall back to opening a
            // separate top-level window.
            if !matches!(
                parent.native_window_handle(),
                Some(raw_window_handle::RawWindowHandle::Xlib(_))
                    | Some(raw_window_handle::RawWindowHandle::Xcb(_))
            ) {
                return Err("The built-in browser needs an X11 session on Linux.".into());
            }
            gtk::init().map_err(|error| format!("Could not start the browser: {error}"))?;
        }

        let storage = storage_dir(session);
        std::fs::create_dir_all(&storage)
            .map_err(|error| format!("Could not prepare browser storage: {error}"))?;
        let mut context = wry::WebContext::new(Some(storage));

        let (nav_page, nav_wake) = (page.clone(), wake.clone());
        let (title_page, title_wake) = (page.clone(), wake.clone());
        let (load_page, load_wake) = (page.clone(), wake.clone());
        let (popup_page, popup_wake) = (page.clone(), wake.clone());
        let (ipc_page, ipc_wake) = (page.clone(), wake.clone());
        let (done_page, done_wake) = (page.clone(), wake.clone());

        #[allow(unused_mut)]
        let mut builder = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_bounds(rect(bounds))
            .with_visible(false)
            .with_focused(false)
            .with_url(session.url.as_str())
            .with_devtools(true)
            .with_accept_first_mouse(true)
            .with_back_forward_navigation_gestures(true)
            .with_background_throttling(wry::BackgroundThrottlingPolicy::Suspend)
            .with_initialization_script(PAGE_SCRIPT)
            .with_initialization_script(super::agent::PAGE_AGENT)
            .with_initialization_script(super::design::SCRIPT)
            .with_initialization_script(FAVICON_SCRIPT)
            .with_navigation_handler(move |url| {
                let allowed = BrowserUrl::page_may_navigate(&url);
                if !allowed {
                    record(&nav_page, &nav_wake, |page| {
                        if super::app_links::scheme(&url).is_some() {
                            page.app_link = Some(url);
                        } else {
                            page.blocked = Some(url);
                        }
                    });
                }
                allowed
            })
            .with_document_title_changed_handler(move |title| {
                record(&title_page, &title_wake, |page| {
                    page.title = title.chars().take(256).collect();
                });
            })
            .with_on_page_load_handler(move |event, url| {
                record(&load_page, &load_wake, |page| {
                    page.loading = matches!(event, wry::PageLoadEvent::Started);
                    page.responded = true;
                    if page.loading {
                        page.edited = false;
                        page.document_sequence = page.document_sequence.wrapping_add(1);
                        page.design_messages.clear();
                    }
                    page.url = Some(url);
                });
            })
            .with_new_window_req_handler(move |url, _| {
                // Links that target a new window open as a new browser tab.
                record(&popup_page, &popup_wake, |page| {
                    if super::app_links::scheme(&url).is_some() {
                        page.app_link = Some(url);
                    } else {
                        page.new_window = Some(url);
                    }
                });
                wry::NewWindowResponse::Deny
            })
            .with_ipc_handler(move |request| {
                let message = request.body().as_str();
                record(&ipc_page, &ipc_wake, |page| match message {
                    "focus" => page.focused = true,
                    "edited" => page.edited = true,
                    "zoom:1" if page.zoom_steps.len() < 8 => page.zoom_steps.push(1),
                    "zoom:-1" if page.zoom_steps.len() < 8 => page.zoom_steps.push(-1),
                    _ if message.starts_with("favicon:") && message.len() <= 2048 => {
                        page.favicon = Some(message[8..].to_string());
                    }
                    _ if message.starts_with("dragent-design:") => {
                        let message = &message[15..];
                        let queued: usize = page.design_messages.iter().map(String::len).sum();
                        if page.design_messages.len() < 8 && queued + message.len() <= super::design::MAX_MESSAGE {
                            page.design_messages.push_back(message.to_string());
                        }
                    }
                    _ => {}
                });
            })
            // What a site may ask for is asked of the person by the engine's
            // own prompt (WebKit's on macOS, WebView2's on Windows; WebKitGTK
            // refuses what nobody answers). Anything else is refused.
            .with_permission_handler(|kind| permission(kind))
            .with_download_started_handler(|url, destination| {
                match download_destination(&url, destination) {
                    Some(path) => {
                        *destination = path;
                        true
                    }
                    None => false,
                }
            })
            .with_download_completed_handler(move |_, path, success| {
                record(&done_page, &done_wake, |page| {
                    page.downloaded = Some(match (success, path) {
                        (true, Some(path)) => Ok(path),
                        _ => Err("The download did not finish.".into()),
                    });
                });
            });

        // The mobile view presents a phone everywhere; a desktop pane on macOS
        // says it is Safari (see `user_agent`). WebView2 and WebKitGTK already
        // name their browser.
        if mobile {
            builder = builder.with_user_agent(super::user_agent::mobile());
        } else if cfg!(target_os = "macos") {
            builder = builder.with_user_agent(super::user_agent::desktop());
        }

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            builder = builder.with_initialization_script(ZOOM_SCRIPT);
        }

        #[cfg(target_os = "macos")]
        {
            use wry::WebViewBuilderExtDarwin;
            let (crash_page, crash_wake) = (page.clone(), wake.clone());
            builder = builder.with_on_web_content_process_terminate_handler(move || {
                record(&crash_page, &crash_wake, |page| page.crashed = true);
            });
            // WKWebView ignores the context's data directory. Per-organisation
            // persistent stores need macOS 14; older systems get a private
            // store so sign-ins never cross organisations.
            if mac::custom_data_stores_available() {
                builder =
                    builder.with_data_store_identifier(*session.storage_partition().as_bytes());
            } else {
                builder = builder.with_incognito(true);
            }
        }

        let view = builder
            .build_as_child(&Parent(parent.clone()))
            .map_err(|error| format!("Could not start the browser: {error}"))?;
        #[cfg(target_os = "macos")]
        let inspector_anchor = {
            use wry::WebViewExtMacOS;
            // Trackpad pinch magnifies the page as it does in Safari.
            mac::allow_magnification(&view.webview());
            mac::keep_inspector_detached(&view.webview())
        };
        Ok(Self {
            view,
            _context: context,
            _parent: parent,
            #[cfg(target_os = "macos")]
            _inspector_anchor: inspector_anchor,
            bounds: None,
            visible: false,
            parked: None,
        })
    }

    /// Move the guest to `bounds` and show or hide it. Hiding returns
    /// keyboard focus to the workspace if the page held it.
    pub fn place(&mut self, bounds: Option<RectF>, show: bool) {
        self.parked = None;
        if let Some(bounds) = bounds {
            if self.bounds != Some(bounds) {
                let _ = self.view.set_bounds(rect(bounds));
                self.bounds = Some(bounds);
            }
        }
        let show = show && bounds.is_some();
        if show && !self.visible {
            let _ = self.view.set_visible(true);
            self.visible = true;
        } else if !show && self.visible {
            if self.has_keyboard_focus() {
                let _ = self.view.focus_parent();
            }
            let _ = self.view.set_visible(false);
            self.visible = false;
        }
    }

    /// Keep the page running where nobody can see or click it: moved far
    /// outside the window, still "visible" to the engine. A hidden page
    /// freezes (timers stop, captures go stale); a parked one doesn't. Used
    /// while a phone drives a tab that the desktop shows covered.
    ///
    /// `size` lays the page out at another size while parked (a phone's
    /// screen, for its mobile view); `None` keeps the pane's own size.
    pub fn park(&mut self, bounds: RectF, size: Option<(f32, f32)>) {
        let size = size
            .map(|(width, height)| warpui::geometry::vector::vec2f(width, height))
            .unwrap_or_else(|| bounds.size());
        let away = RectF::new(
            warpui::geometry::vector::vec2f(bounds.origin_x() + 30_000.0, bounds.origin_y()),
            size,
        );
        if self.parked == Some(away) {
            return;
        }
        self.parked = Some(away);
        if self.has_keyboard_focus() {
            let _ = self.view.focus_parent();
        }
        let _ = self.view.set_bounds(rect(away));
        // Not where `place` last put it: the next `place` moves it back.
        self.bounds = None;
        if !self.visible {
            let _ = self.view.set_visible(true);
            self.visible = true;
        }
    }

    /// Whether the page can be captured, for showing it under a menu.
    pub fn can_capture(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
    }

    /// The visible page as an image, to draw while something covers it.
    pub fn capture(&self, done: Box<dyn FnOnce(Option<warpui::image_cache::StaticImage>)>) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            mac::snapshot(
                &self.view.webview(),
                Box::new(move |image| {
                    done(image.and_then(|image| unsafe { mac::rgba(&image) }).and_then(
                        |(width, height, pixels)| {
                            warpui::image_cache::StaticImage::from_rgba(width, height, pixels)
                        },
                    ))
                }),
            );
        }
        #[cfg(target_os = "windows")]
        self.screenshot(Box::new(move |png| {
            done(png.ok().and_then(|png| {
                let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                    .ok()?
                    .into_rgba8();
                let (width, height) = image.dimensions();
                warpui::image_cache::StaticImage::from_rgba(width, height, image.into_raw())
            }));
        }));
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        done(None);
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn navigate(&self, url: &BrowserUrl) -> Result<(), String> {
        self.view
            .load_url(url.as_str())
            .map_err(|error| error.to_string())
    }

    pub fn back(&self) {
        let _ = self.view.go_back();
    }

    pub fn forward(&self) {
        let _ = self.view.go_forward();
    }

    pub fn reload(&self) {
        let _ = self.view.reload();
    }

    /// Reload bypassing the cache.
    pub fn hard_reload(&self) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            mac::reload_from_origin(&self.view.webview());
        }
        #[cfg(not(target_os = "macos"))]
        let _ = self.view.evaluate_script("location.reload()");
    }

    /// Whether back and forward have somewhere to go. Platforms without a
    /// query report both as available.
    pub fn history(&self) -> (bool, bool) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            mac::history(&self.view.webview())
        }
        #[cfg(not(target_os = "macos"))]
        (true, true)
    }

    pub fn set_zoom(&self, factor: f64) {
        let _ = self.view.zoom(factor);
    }

    /// Run `script` in the page; `done` gets its value, JSON-encoded.
    pub fn eval(&self, script: &str, done: impl Fn(String) + Send + 'static) -> Result<(), String> {
        self.view
            .evaluate_script_with_callback(script, done)
            .map_err(|error| format!("The page couldn't run the command: {error}"))
    }

    pub fn script(&self, script: &str) -> Result<(), String> {
        self.view.evaluate_script(script).map_err(|error| error.to_string())
    }

    /// A PNG of the visible page at most `width` points wide: much less to
    /// encode than the full-resolution capture, for a phone's live view.
    pub fn screenshot_small(&self, width: f64, done: Box<dyn FnOnce(Result<Vec<u8>, String>)>) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            let page_width = self.parked.or(self.bounds).map(|b| b.width() as f64).unwrap_or(width);
            mac::snapshot_width(
                &self.view.webview(),
                Some(width.min(page_width.max(1.0))),
                Box::new(move |image| {
                    done(image
                        .and_then(|image| unsafe { mac::png(&image) })
                        .ok_or_else(|| "The page couldn't be captured.".to_string()))
                }),
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = width;
            self.screenshot(done);
        }
    }

    /// A PNG of the visible page.
    pub fn screenshot(&self, done: Box<dyn FnOnce(Result<Vec<u8>, String>)>) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            mac::snapshot(
                &self.view.webview(),
                Box::new(move |image| {
                    done(image
                        .and_then(|image| unsafe { mac::png(&image) })
                        .ok_or_else(|| "The page couldn't be captured.".to_string()))
                }),
            );
        }
        #[cfg(target_os = "linux")]
        {
            use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
            use wry::WebViewExtUnix;
            self.view.webview().snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE,
                None::<&gtk::gio::Cancellable>, move |result| {
                    done(result.map_err(|error| error.to_string()).and_then(|surface| {
                        let mut bytes = Vec::new();
                        surface.write_to_png(&mut bytes).map_err(|error| error.to_string())?;
                        Ok(bytes)
                    }));
                });
        }
        #[cfg(target_os = "windows")]
        {
            use webview2_com::{CapturePreviewCompletedHandler, Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG};
            use windows::Win32::System::Com::{StructuredStorage::CreateStreamOnHGlobal, STATSTG, STATFLAG_NONAME, STREAM_SEEK_SET};
            use wry::WebViewExtWindows;
            let done = Rc::new(RefCell::new(Some(done)));
            let complete = done.clone();
            let result = (|| -> windows::core::Result<()> {
                let stream = unsafe { CreateStreamOnHGlobal(Default::default(), true)? };
                let output = stream.clone();
                let handler = CapturePreviewCompletedHandler::create(Box::new(move |status| {
                    let result = (|| -> Result<Vec<u8>, String> {
                        status.map_err(|error| error.to_string())?;
                        let mut stat = STATSTG::default();
                        unsafe { output.Stat(&mut stat, STATFLAG_NONAME).map_err(|e| e.to_string())?; }
                        if stat.cbSize > 32 * 1024 * 1024 { return Err("The page capture is too large.".into()); }
                        let mut bytes = vec![0; stat.cbSize as usize];
                        let mut read = 0;
                        unsafe {
                            output.Seek(0, STREAM_SEEK_SET, None).map_err(|e| e.to_string())?;
                            output.Read(bytes.as_mut_ptr().cast(), bytes.len() as u32, Some(&mut read)).ok().map_err(|e| e.to_string())?;
                        }
                        bytes.truncate(read as usize);
                        Ok(bytes)
                    })();
                    if let Some(done) = complete.borrow_mut().take() { done(result); }
                    Ok(())
                }));
                unsafe { self.view.webview().CapturePreview(COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, &stream, &handler) }
            })();
            if let Err(error) = result {
                if let Some(done) = done.borrow_mut().take() { done(Err(error.to_string())); }
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        done(Err("Page capture isn't available on this platform.".into()));
    }

    pub fn print(&self) {
        let _ = self.view.print();
    }

    /// Add one imported cookie to this pane's profile (shared by every pane
    /// of the organisation).
    pub fn import_cookie(&self, imported: &dragent_host::browser_cookies::ImportedCookie) -> Result<(), String> {
        use dragent_host::browser_cookies::SameSite;
        let mut cookie = cookie::Cookie::build((imported.name.clone(), imported.value.clone()))
            .domain(imported.domain.clone())
            .path(if imported.path.is_empty() { "/".to_string() } else { imported.path.clone() })
            .secure(imported.secure)
            .http_only(imported.http_only);
        if let Some(same_site) = imported.same_site {
            cookie = cookie.same_site(match same_site {
                SameSite::None => cookie::SameSite::None,
                SameSite::Lax => cookie::SameSite::Lax,
                SameSite::Strict => cookie::SameSite::Strict,
            });
        }
        if let Some(at) = imported.expires.and_then(|at| time::OffsetDateTime::from_unix_timestamp(at).ok()) {
            cookie = cookie.expires(at);
        }
        self.view.set_cookie(&cookie.build()).map_err(|error| error.to_string())
    }

    /// Cookies, cache and site storage of this pane's profile: every pane of
    /// the same organisation shares them.
    pub fn clear_browsing_data(&self) -> Result<(), String> {
        self.view
            .clear_all_browsing_data()
            .map_err(|error| format!("Couldn't clear browsing data: {error}"))
    }

    pub fn stop(&self) {
        let _ = self.view.evaluate_script("window.stop()");
    }

    pub fn devtools(&self) {
        self.view.open_devtools();
    }

    /// Fallback for WebKit versions without an attachment anchor: move a
    /// docked inspector into its own window, then restore the page's frame
    /// (docking resizes it to fill the workspace window).
    pub fn undock_devtools(&mut self) {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            mac::detach_inspector(&self.view.webview());
        }
        if let Some(bounds) = self.bounds.take() {
            let visible = self.visible;
            self.place(Some(bounds), visible);
        }
    }

    pub fn focus(&self) {
        if self.visible && !self.has_keyboard_focus() {
            let _ = self.view.focus();
        }
    }

    pub fn release_focus(&self) {
        if self.has_keyboard_focus() {
            let _ = self.view.focus_parent();
        }
    }

    /// Whether the page currently receives keystrokes. Checked synchronously
    /// so hiding one pane never takes focus from a pane that just gained it.
    fn has_keyboard_focus(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            use wry::WebViewExtMacOS;
            mac::is_first_responder(&self.view.webview())
        }
        #[cfg(not(target_os = "macos"))]
        {
            // WebView2 and WebKitGTK keep focus inside their own child
            // window; returning focus to the parent is always safe there.
            self.visible
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl Drop for Engine {
    fn drop(&mut self) {
        self.release_focus();
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_app_kit::{NSResponder, NSView};
    use objc2_foundation::{NSOperatingSystemVersion, NSProcessInfo};

    pub fn custom_data_stores_available() -> bool {
        NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
            majorVersion: 14,
            minorVersion: 0,
            patchVersion: 0,
        })
    }

    /// WebKit docks the inspector only when its attachment view is tall
    /// enough. A zero-size anchor makes it always open in its own window
    /// (and disables its dock buttons) instead of covering the workspace.
    /// The anchor must stay alive as long as the web view.
    pub fn keep_inspector_detached(webview: &NSView) -> Option<Retained<NSView>> {
        use objc2::runtime::Bool;
        use objc2::{msg_send, sel, MainThreadMarker};
        let main_thread = MainThreadMarker::new()?;
        unsafe {
            let responds: Bool =
                msg_send![webview, respondsToSelector: sel!(_setInspectorAttachmentView:)];
            if !responds.as_bool() {
                return None;
            }
            let anchor = NSView::new(main_thread);
            let _: () = msg_send![webview, _setInspectorAttachmentView: &*anchor];
            Some(anchor)
        }
    }

    /// Move an open Web Inspector out of the workspace window into its own
    /// window. Skipped if this WebKit does not offer it.
    pub fn detach_inspector(webview: &NSView) {
        use objc2::runtime::{AnyObject, Bool};
        use objc2::{msg_send, sel};
        unsafe {
            let responds: Bool = msg_send![webview, respondsToSelector: sel!(_inspector)];
            if !responds.as_bool() {
                return;
            }
            let inspector: *mut AnyObject = msg_send![webview, _inspector];
            let Some(inspector) = inspector.as_ref() else {
                return;
            };
            let responds: Bool = msg_send![inspector, respondsToSelector: sel!(detach)];
            if responds.as_bool() {
                let _: () = msg_send![inspector, detach];
            }
        }
    }

    /// Snapshot the page as WebKit renders it; `done` gets the `NSImage`.
    pub fn snapshot(webview: &NSView, done: Box<dyn FnOnce(Option<Retained<AnyObject>>)>) {
        snapshot_width(webview, None, done)
    }

    /// A snapshot scaled to `width` points wide, or full size for `None`.
    pub fn snapshot_width(
        webview: &NSView,
        width: Option<f64>,
        done: Box<dyn FnOnce(Option<Retained<AnyObject>>)>,
    ) {
        use std::cell::RefCell;

        use block2::RcBlock;

        let done = RefCell::new(Some(done));
        let block = RcBlock::new(move |image: *mut AnyObject, _error: *mut AnyObject| {
            if let Some(done) = done.borrow_mut().take() {
                done(unsafe { Retained::retain(image) });
            }
        });
        unsafe {
            // Held until the call returns; WebKit copies what it needs.
            let configured: Option<Retained<AnyObject>> = width.map(|width| {
                let configuration: Retained<AnyObject> = msg_send![class!(WKSnapshotConfiguration), new];
                let number: Retained<AnyObject> = msg_send![class!(NSNumber), numberWithDouble: width];
                let _: () = msg_send![&*configuration, setSnapshotWidth: &*number];
                configuration
            });
            let configuration: *const AnyObject =
                configured.as_deref().map_or(std::ptr::null(), |c| c as *const AnyObject);
            let _: () = msg_send![
                webview,
                takeSnapshotWithConfiguration: configuration,
                completionHandler: &*block
            ];
        }
    }

    /// The image as PNG.
    pub unsafe fn png(image: &AnyObject) -> Option<Vec<u8>> {
        /// `NSBitmapImageFileTypePNG`.
        const PNG: usize = 4;
        let tiff: *mut AnyObject = msg_send![image, TIFFRepresentation];
        let rep: *mut AnyObject =
            msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff.as_ref()?];
        let properties: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
        let data: *mut AnyObject = msg_send![
            rep.as_ref()?,
            representationUsingType: PNG,
            properties: properties.as_ref()?
        ];
        let data = data.as_ref()?;
        let length: usize = msg_send![data, length];
        let bytes: *const u8 = msg_send![data, bytes];
        (!bytes.is_null()).then(|| std::slice::from_raw_parts(bytes, length).to_vec())
    }

    /// The image's pixels as tightly packed RGBA8, decoded by ImageIO and
    /// drawn by Core Graphics (no compression round trip).
    pub unsafe fn rgba(image: &AnyObject) -> Option<(u32, u32, Vec<u8>)> {
        use std::ffi::c_void;

        #[repr(C)]
        struct CGRect {
            x: f64,
            y: f64,
            width: f64,
            height: f64,
        }
        #[link(name = "ImageIO", kind = "framework")]
        extern "C" {
            fn CGImageSourceCreateWithData(data: *const c_void, options: *const c_void)
                -> *mut c_void;
            fn CGImageSourceCreateImageAtIndex(
                source: *mut c_void,
                index: usize,
                options: *const c_void,
            ) -> *mut c_void;
        }
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGImageGetWidth(image: *mut c_void) -> usize;
            fn CGImageGetHeight(image: *mut c_void) -> usize;
            fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
            fn CGColorSpaceRelease(space: *mut c_void);
            fn CGBitmapContextCreate(
                data: *mut c_void,
                width: usize,
                height: usize,
                bits_per_component: usize,
                bytes_per_row: usize,
                space: *mut c_void,
                bitmap_info: u32,
            ) -> *mut c_void;
            fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *mut c_void);
            fn CGContextRelease(context: *mut c_void);
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFRelease(object: *mut c_void);
        }
        /// `kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big`: RGBA.
        const RGBA: u32 = 1 | (4 << 12);

        let tiff: *mut AnyObject = msg_send![image, TIFFRepresentation];
        if tiff.is_null() {
            return None;
        }
        let source = CGImageSourceCreateWithData(tiff as *const c_void, std::ptr::null());
        if source.is_null() {
            return None;
        }
        let cg = CGImageSourceCreateImageAtIndex(source, 0, std::ptr::null());
        CFRelease(source);
        if cg.is_null() {
            return None;
        }
        let (width, height) = (CGImageGetWidth(cg), CGImageGetHeight(cg));
        let mut pixels = vec![0u8; width * height * 4];
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(
            pixels.as_mut_ptr() as *mut c_void,
            width,
            height,
            8,
            width * 4,
            space,
            RGBA,
        );
        CGColorSpaceRelease(space);
        if context.is_null() {
            CFRelease(cg);
            return None;
        }
        let rect = CGRect {
            x: 0.0,
            y: 0.0,
            width: width as f64,
            height: height as f64,
        };
        CGContextDrawImage(context, rect, cg);
        CGContextRelease(context);
        CFRelease(cg);
        Some((width as u32, height as u32, pixels))
    }

    pub fn reload_from_origin(webview: &NSView) {
        use objc2::msg_send;
        use objc2::runtime::AnyObject;
        unsafe {
            let _: *mut AnyObject = msg_send![webview, reloadFromOrigin];
        }
    }

    pub fn history(webview: &NSView) -> (bool, bool) {
        use objc2::msg_send;
        use objc2::runtime::Bool;
        unsafe {
            let back: Bool = msg_send![webview, canGoBack];
            let forward: Bool = msg_send![webview, canGoForward];
            (back.as_bool(), forward.as_bool())
        }
    }

    pub fn allow_magnification(webview: &NSView) {
        use objc2::runtime::Bool;
        use objc2::{msg_send, sel};
        unsafe {
            let responds: Bool =
                msg_send![webview, respondsToSelector: sel!(setAllowsMagnification:)];
            if responds.as_bool() {
                let _: () = msg_send![webview, setAllowsMagnification: Bool::YES];
            }
        }
    }

    pub fn is_first_responder(webview: &NSView) -> bool {
        let Some(window) = webview.window() else {
            return false;
        };
        let Some(responder): Option<Retained<NSResponder>> = window.firstResponder() else {
            return false;
        };
        // WKWebView focuses an internal subview, so test the whole subtree.
        responder
            .downcast::<NSView>()
            .is_ok_and(|view| view.isDescendantOf(webview))
    }
}

#[cfg(not(target_family = "wasm"))]
fn rect(bounds: RectF) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(
            bounds.origin_x() as f64,
            bounds.origin_y() as f64,
        )
        .into(),
        size: wry::dpi::LogicalSize::new(bounds.width() as f64, bounds.height() as f64).into(),
    }
}

/// What a page may ask the person for through the engine's own prompt.
#[cfg(not(target_family = "wasm"))]
fn permission(kind: wry::PermissionKind) -> wry::PermissionResponse {
    use wry::PermissionKind as Kind;
    match kind {
        Kind::Camera
        | Kind::Microphone
        | Kind::Geolocation
        | Kind::Notifications
        | Kind::DisplayCapture
        | Kind::ClipboardRead => wry::PermissionResponse::Default,
        _ => wry::PermissionResponse::Deny,
    }
}

/// One store per organisation profile, under the organisation's own folder.
pub(super) fn storage_dir(session: &BrowserPaneSession) -> PathBuf {
    dragent_paths::organisation_dir_for(&session.owner.organisation)
        .join("browser")
        .join(session.storage_partition().to_string())
}

/// Where pages download to, and screenshots are saved.
pub(super) fn downloads_folder() -> Option<PathBuf> {
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
}

/// A unique file in `folder` named `name`, numbered when it is taken.
pub(super) fn unique_file(folder: &Path, name: &str) -> Option<PathBuf> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (name, None),
    };
    (0..1000).find_map(|attempt| {
        let file = match (attempt, extension) {
            (0, _) => name.to_string(),
            (n, Some(extension)) => format!("{stem} ({n}).{extension}"),
            (n, None) => format!("{stem} ({n})"),
        };
        let path = folder.join(file);
        (!path.exists()).then_some(path)
    })
}

/// A unique file in the user's Downloads folder, named after what the site
/// suggested (or the URL) but never allowed to escape that folder.
fn download_destination(url: &str, suggested: &Path) -> Option<PathBuf> {
    let folder = downloads_folder()?;
    let from_url = || {
        url::Url::parse(url)
            .ok()?
            .path_segments()?
            .next_back()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    };
    let name = suggested
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .or_else(from_url)
        .unwrap_or_else(|| "download".into());
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_start_matches('.');
    let name = if name.is_empty() { "download" } else { name };
    unique_file(&folder, name)
}

#[cfg(target_family = "wasm")]
pub(super) struct Engine;

#[cfg(target_family = "wasm")]
impl Engine {
    pub fn create(
        _: Rc<dyn warpui::platform::Window>,
        _: &BrowserPaneSession,
        _: RectF,
        _: Page,
        _: Sender<()>,
        _: bool,
    ) -> Result<Self, String> {
        Err("The built-in browser is available in the desktop app.".into())
    }
    pub fn place(&mut self, _: Option<RectF>, _: bool) {}
    pub fn park(&mut self, _: RectF, _: Option<(f32, f32)>) {}
    pub fn screenshot_small(&self, _: f64, done: Box<dyn FnOnce(Result<Vec<u8>, String>)>) {
        self.screenshot(done);
    }
    pub fn can_capture(&self) -> bool {
        false
    }
    pub fn is_visible(&self) -> bool {
        false
    }
    pub fn navigate(&self, _: &BrowserUrl) -> Result<(), String> {
        Err("The built-in browser is available in the desktop app.".into())
    }
    pub fn back(&self) {}
    pub fn forward(&self) {}
    pub fn reload(&self) {}
    pub fn hard_reload(&self) {}
    pub fn history(&self) -> (bool, bool) {
        (false, false)
    }
    pub fn set_zoom(&self, _: f64) {}
    pub fn eval(&self, _: &str, _: impl Fn(String) + Send + 'static) -> Result<(), String> {
        Err("The built-in browser is available in the desktop app.".into())
    }
    pub fn script(&self, _: &str) -> Result<(), String> { Ok(()) }
    pub fn screenshot(&self, done: Box<dyn FnOnce(Result<Vec<u8>, String>)>) {
        done(Err(
            "The built-in browser is available in the desktop app.".into()
        ));
    }
    pub fn capture(&self, done: Box<dyn FnOnce(Option<warpui::image_cache::StaticImage>)>) {
        done(None);
    }
    pub fn print(&self) {}
    pub fn clear_browsing_data(&self) -> Result<(), String> {
        Ok(())
    }
    pub fn stop(&self) {}
    pub fn devtools(&self) {}
    pub fn undock_devtools(&mut self) {}
    pub fn focus(&self) {}
    pub fn release_focus(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_stay_in_the_downloads_folder_with_unique_names() {
        let Some(folder) = dirs::download_dir() else {
            return;
        };
        let path = download_destination(
            "https://example.com/files/report.pdf",
            Path::new("../../etc/passwd"),
        )
        .unwrap();
        assert_eq!(path.parent(), Some(folder.as_path()));
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("passwd"));

        let path =
            download_destination("https://example.com/files/report.pdf", Path::new("")).unwrap();
        assert_eq!(path.parent(), Some(folder.as_path()));
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("report"));

        let hidden = download_destination("https://example.com/", Path::new(".bashrc")).unwrap();
        assert!(!hidden
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with('.'));
    }
}
