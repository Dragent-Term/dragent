use super::*;
use crate::browser::design::{self, DesignEvent, DesignNote, DesignSelection};
use crate::workspace::AgentTarget;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use warpui_core::clipboard::ImageData;

impl BrowserView {
    pub(crate) fn set_design_targets(&mut self, targets: Vec<AgentTarget>, ctx: &mut ViewContext<Self>) {
        self.design.targets = targets;
        let listed = self
            .design
            .targets
            .iter()
            .any(|target| Some(&target.key) == self.design.target.as_ref());
        if !listed || !self.design.target_chosen {
            self.design.target = crate::workspace::default_agent_target(
                &self.design.targets,
                &crate::setup::UiSettings::load(),
            );
        }
        self.sync_design();
        ctx.notify();
    }

    pub fn finish_design_send(&mut self, result: Result<(), String>, ctx: &mut ViewContext<Self>) {
        if result.is_ok() {
            let sent = std::mem::take(&mut self.design.sending);
            self.design.notes.retain(|note| !sent.contains(&note.id));
            // The next batch defaults afresh, e.g. to a session this one
            // started, once that session is up.
            self.design.target_chosen = false;
            self.design.save();
            ctx.emit(BrowserEvent::DesignTargetsRequested);
            ctx.spawn(Timer::after(Duration::from_millis(1500)), |_, _, ctx| {
                ctx.emit(BrowserEvent::DesignTargetsRequested)
            });
        } else {
            self.design.sending.clear();
        }
        self.sync_design();
        self.design_status(
            result
                .err()
                .as_deref()
                .unwrap_or("Feedback sent to your agent."),
        );
        ctx.notify();
    }

    pub(super) fn sync_design(&self) {
        let config = serde_json::json!({
            "enabled": self.design.enabled && self.active,
            "token": self.design.token,
            "notes": self.design.notes,
            "target": self.design_target().map(AgentTarget::chip_label),
            "sending": !self.design.sending.is_empty(),
            "dark": self.theme.is_dark,
            "palette": design_palette(&self.theme),
        });
        self.with_engine(|engine| {
            let _ = engine.script(&format!("window.__dragentDesign?.configure({config})"));
        });
    }

    pub(super) fn design_target(&self) -> Option<&AgentTarget> {
        self.design
            .targets
            .iter()
            .find(|target| Some(&target.key) == self.design.target.as_ref())
    }

    pub(super) fn select_design_target(&mut self, key: &str, ctx: &mut ViewContext<Self>) {
        if self.design.targets.iter().any(|t| t.key == key) {
            self.design.target = Some(key.to_string());
            self.design.target_chosen = true;
            crate::workspace::remember_agent_target(key);
            self.sync_design();
            ctx.notify();
        }
    }

    pub(super) fn design_draw(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.design.enabled {
            return;
        }
        self.with_engine(|engine| {
            let _ = engine.script("window.__dragentDesign?.tool('draw')");
        });
        self.with_engine(Engine::focus);
        ctx.notify();
    }

    fn design_status(&self, text: &str) {
        self.with_engine(|engine| {
            let text = serde_json::to_string(text).unwrap_or_default();
            let _ = engine.script(&format!("window.__dragentDesign?.error({text})"));
        });
    }

    pub(super) fn toggle_design(&mut self, ctx: &mut ViewContext<Self>) {
        if !self.has_page() {
            return;
        }
        self.design.enabled = !self.design.enabled;
        self.menu_open = false;
        self.design_menu_open = false;
        if self.design.enabled {
            ctx.emit(BrowserEvent::DesignTargetsRequested);
        }
        self.sync_design();
        self.with_engine(Engine::focus);
        ctx.notify();
    }

    pub(super) fn design_message(&mut self, raw: &str, ctx: &mut ViewContext<Self>) {
        let Ok(message) = serde_json::from_str::<design::Message>(raw) else {
            return;
        };
        if message.token != self.design.token {
            return;
        }
        if (!self.design.enabled || !self.active)
            && !matches!(&message.event, DesignEvent::Update { .. })
        {
            return;
        }
        let result = match message.event {
            DesignEvent::Ready => {
                ctx.emit(BrowserEvent::DesignTargetsRequested);
                Ok(())
            }
            DesignEvent::Close => {
                self.design.enabled = false;
                self.design_menu_open = false;
                self.sync_design();
                ctx.notify();
                Ok(())
            }
            DesignEvent::Pick { selection } => self.capture_design(selection, false, ctx),
            DesignEvent::CaptureMarkup { selection } => self.capture_design(selection, true, ctx),
            DesignEvent::Markup { image, selection } => {
                let result = selection.validate();
                if let Err(error) = result {
                    Err(error)
                } else if self.design.capturing || self.design.notes.len() >= design::MAX_NOTES {
                    Err("Finish the current capture or send some notes first.".into())
                } else {
                    let png = image
                        .strip_prefix("data:image/png;base64,")
                        .ok_or_else(|| "The marked capture isn't a PNG.".to_string())
                        .and_then(|data| STANDARD.decode(data).map_err(|e| e.to_string()));
                    match png {
                        Ok(png) => {
                            self.save_design_note(selection, Ok(png), true, ctx);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
            }
            DesignEvent::Update {
                id,
                comment,
                intent,
            } => {
                if comment.len() > 8192
                    || !["change", "fix", "question", "remove"].contains(&intent.as_str())
                {
                    Err("The feedback is too long or has an unknown intent.".into())
                } else if self.design.sending.contains(&id) {
                    Err("Wait for this feedback to finish sending before editing it.".into())
                } else {
                    if let Some(note) = self.design.notes.iter_mut().find(|n| n.id == id) {
                        note.comment = comment;
                        note.intent = intent;
                        self.design.save();
                    }
                    Ok(())
                }
            }
            DesignEvent::Remove { id } => {
                if self.design.sending.contains(&id) {
                    Err("This feedback is being sent.".into())
                } else {
                    self.design.notes.retain(|n| n.id != id);
                    self.design.save();
                    self.sync_design();
                    Ok(())
                }
            }
            DesignEvent::Clear => {
                if !self.design.sending.is_empty() {
                    Err("Wait for feedback to finish sending before clearing it.".into())
                } else {
                    self.design.notes.clear();
                    self.design.save();
                    self.sync_design();
                    Ok(())
                }
            }
            DesignEvent::Copy => {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(self.design.context()));
                self.design_status("Copied every note with its page context.");
                Ok(())
            }
            DesignEvent::CopyNote { id } => {
                match self.design.notes.iter().position(|n| n.id == id) {
                    Some(index) => {
                        let text = self.design.note_context(index, &self.design.notes[index]);
                        ctx.clipboard().write(ClipboardContent::plain_text(text));
                        self.design_status("Copied this note with its page context.");
                        Ok(())
                    }
                    None => Err("That note was deleted.".into()),
                }
            }
            DesignEvent::CopyElement { selection } => selection.validate().map(|()| {
                ctx.clipboard()
                    .write(ClipboardContent::plain_text(design::selection_context(&selection)));
                self.design_status("Copied the element's HTML, CSS, and location.");
            }),
            DesignEvent::CopyImage { id } => {
                if self.design.notes.iter().any(|n| n.id == id && n.screenshot) {
                    self.copy_design_image(id, ctx);
                    Ok(())
                } else {
                    Err("This note has no screenshot.".into())
                }
            }
            DesignEvent::Flushed { request } => {
                if self.design.send_request == Some(request) {
                    self.design.send_request = None;
                    self.deliver_design(ctx);
                }
                Ok(())
            }
        };
        if let Err(error) = result {
            self.design_status(&error);
        }
    }

    // Sending requires an app-chrome action; guest IPC may only edit reference material.
    pub(super) fn send_design(&mut self, ctx: &mut ViewContext<Self>) {
        self.design_menu_open = false;
        if !self.design.enabled
            || self.design.capturing
            || !self.design.sending.is_empty()
            || self.design.notes.is_empty()
            || self.design.send_request.is_some()
        {
            return;
        }
        let request = uuid::Uuid::new_v4();
        self.design.send_request = Some(request);
        self.with_engine(|engine| {
            let _ = engine.script(&format!("window.__dragentDesign?.flush('{request}')"));
        });
        ctx.spawn(Timer::after(Duration::from_secs(3)), move |view, _, ctx| {
            if view.design.send_request == Some(request) {
                view.design.send_request = None;
                view.design_status(
                    "The page changed before feedback could be sent. Open Design and try again.",
                );
                ctx.notify();
            }
        });
    }

    fn deliver_design(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(target) = self
            .design
            .target
            .clone()
            .filter(|key| self.design.targets.iter().any(|t| &t.key == key))
        else {
            self.design_menu_open = true;
            ctx.emit(BrowserEvent::DesignTargetsRequested);
            self.design_status("Choose an agent to send your notes to.");
            return;
        };
        if let Some(index) = self
            .design
            .notes
            .iter()
            .position(|n| n.comment.trim().is_empty())
        {
            self.with_engine(|engine| {
                let id = self.design.notes[index].id;
                let _ = engine.script(&format!("window.__dragentDesign?.open('{id}')"));
            });
            self.design_status(&format!(
                "Note {} has no request yet. Describe the change, then send.",
                index + 1
            ));
            return;
        }
        self.design.sending = self.design.notes.iter().map(|n| n.id).collect();
        // The agent gets a short brief; the bulky DOM and CSS go in a file it
        // can read, so a terminal agent's input isn't flooded.
        let context = self.design.context();
        let path = self
            .design
            .folder
            .join(format!("feedback-{}.md", uuid::Uuid::new_v4()));
        let brief = self.design.brief(&path);
        let files = self.design.files();
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let result = (|| -> std::io::Result<()> {
                if let Some(parent) = path.parent() {
                    design::private_folder(parent)?;
                }
                std::fs::write(path, context)
            })()
            .map_err(|e| format!("Couldn't save the full feedback: {e}"));
            let _ = tx.send(result);
        });
        ctx.spawn(rx, move |view, result, ctx| match result {
            Ok(Ok(())) => ctx.emit(BrowserEvent::SendDesign {
                target,
                prompt: brief,
                files,
            }),
            _ => view.finish_design_send(
                Err("Couldn't save the feedback context. Your notes are still here.".into()),
                ctx,
            ),
        });
        self.sync_design();
        ctx.notify();
    }

    fn copy_design_image(&self, id: uuid::Uuid, ctx: &mut ViewContext<Self>) {
        let path = self.design.image_path(id);
        // Only the macOS clipboard takes images so far; elsewhere hand over
        // the file instead of silently copying nothing.
        if !cfg!(target_os = "macos") {
            ctx.clipboard()
                .write(ClipboardContent::plain_text(path.display().to_string()));
            self.design_status("Copied the screenshot's file path.");
            return;
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let _ = tx.send(std::fs::read(path));
        });
        ctx.spawn(rx, |view, result, ctx| match result {
            Ok(Ok(data)) => {
                ctx.clipboard().write(ClipboardContent {
                    images: Some(vec![ImageData {
                        data,
                        mime_type: "image/png".into(),
                        filename: Some("design-note.png".into()),
                    }]),
                    ..Default::default()
                });
                view.design_status("Copied the screenshot.");
            }
            _ => view.design_status("Couldn't read the screenshot."),
        });
    }

    fn capture_design(
        &mut self,
        selection: DesignSelection,
        markup: bool,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), String> {
        selection.validate()?;
        let same_origin = url::Url::parse(&selection.url)
            .ok()
            .zip(url::Url::parse(self.session.url.as_str()).ok())
            .is_some_and(|(picked, current)| picked.origin() == current.origin());
        if !same_origin {
            return Err("The page changed. Pick the element again.".into());
        }
        if self.design.capturing || self.design.notes.len() >= design::MAX_NOTES {
            return Err("Finish the current capture or send some notes first.".into());
        }
        let sequence = self.begin_design_capture(ctx);
        let token = self.design.token.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        let capture: Box<dyn FnOnce(Result<Vec<u8>, String>)> = Box::new(move |result| {
            let _ = tx.send(result);
        });
        match self.surface.borrow().engine.as_ref() {
            Some(engine) => engine.screenshot(capture),
            None => capture(Err("The page is no longer visible.".into())),
        }
        ctx.spawn(rx, move |view, result, ctx| {
            if token != view.design.token
                || sequence != view.design.capture_sequence
                || !view.design.capturing
            {
                return;
            }
            let result = result.unwrap_or_else(|_| Err("The page capture was interrupted.".into()));
            if markup {
                view.design.capturing = false;
                match result {
                    Ok(png) => {
                        let selection = serde_json::to_string(&selection).unwrap_or_default();
                        let data = format!("data:image/png;base64,{}", STANDARD.encode(png));
                        view.with_engine(|engine| {
                            let data = serde_json::to_string(&data).unwrap_or_default();
                            let _ = engine.script(&format!(
                                "window.__dragentDesign?.captured({data}, {selection})"
                            ));
                        });
                    }
                    Err(error) => view.design_status(&error),
                }
            } else {
                view.save_design_note(selection, result, false, ctx);
            }
        });
        Ok(())
    }

    fn begin_design_capture(&mut self, ctx: &mut ViewContext<Self>) -> u64 {
        self.design.capturing = true;
        self.design.capture_sequence = self.design.capture_sequence.wrapping_add(1);
        let sequence = self.design.capture_sequence;
        ctx.spawn(Timer::after(Duration::from_secs(8)), move |view, _, ctx| {
            if view.design.capture_sequence == sequence && view.design.capturing {
                view.design.capturing = false;
                view.design.capture_sequence = view.design.capture_sequence.wrapping_add(1);
                view.design_status("The page capture timed out. Try again.");
                ctx.notify();
            }
        });
        sequence
    }

    fn save_design_note(
        &mut self,
        selection: DesignSelection,
        png: Result<Vec<u8>, String>,
        markup: bool,
        ctx: &mut ViewContext<Self>,
    ) {
        let sequence = self.begin_design_capture(ctx);
        let id = uuid::Uuid::new_v4();
        let path = self.design.image_path(id);
        let token = self.design.token.clone();
        let save_selection = selection.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        crate::bg::detach(move || {
            let result =
                png.and_then(|png| design::save_capture(&png, &save_selection, !markup, &path));
            let _ = tx.send(result);
        });
        ctx.spawn(rx, move |view, result, ctx| {
            if token != view.design.token
                || sequence != view.design.capture_sequence
                || !view.design.capturing
            {
                return;
            }
            view.design.capturing = false;
            let result = result.unwrap_or_else(|_| Err("The capture worker stopped.".into()));
            let (preview, capture_error) = match result {
                Ok(preview) => (preview, None),
                Err(error) => (String::new(), Some(error)),
            };
            view.design.notes.push(DesignNote {
                id,
                selection,
                comment: String::new(),
                intent: "change".into(),
                screenshot: capture_error.is_none(),
                capture_error,
                markup,
                preview,
            });
            view.design.save();
            view.sync_design();
            ctx.notify();
        });
    }
}

/// The overlay's colours, from the app theme. Panels are opaque even under a
/// background-image theme; text keeps the theme's own dimming.
fn design_palette(theme: &Theme) -> serde_json::Value {
    let css = |c: ColorU| {
        format!(
            "rgba({},{},{},{:.3})",
            c.r,
            c.g,
            c.b,
            f32::from(c.a) / 255.0
        )
    };
    let opaque = |c: ColorU| css(ColorU::new(c.r, c.g, c.b, 255));
    let solid = Theme {
        background: theme.solid_background(),
        ..*theme
    };
    serde_json::json!({
        "bg": opaque(solid.surface_2()),
        "raised": opaque(solid.surface_3()),
        "fg": css(theme.main_text()),
        "muted": css(theme.sub_text()),
        "line": opaque(solid.border()),
        "accent": opaque(theme.accent),
        "accent_fg": opaque(theme.accent_text()),
        "danger": opaque(theme.ui_red()),
        "warning": opaque(theme.ui_yellow()),
    })
}
