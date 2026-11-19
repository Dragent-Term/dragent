//! Reusable text-input box for small inputs across the app
//! (settings searches, provider keys, modals, commit message, find boxes,
//! port/project/explorer prompts …).
//!
//! The reference design is the settings-sidebar search: a leading icon plus
//! the [`InputEditor`] child view (and an optional parent-built trailing
//! element such as a clear button) inside one themed container. Every visual
//! property comes from [`Theme`] unless the caller overrides it through
//! [`SharedInputOpts`]; placeholder, label and value live on the editor
//! itself. Callers keep ownership of their editor views (state + actions stay
//! in the parent) — this component only draws the shared box around them.
//!
//! Text vertical centering comes from the centered row layout here plus the
//! caret-centering fix in the plain editor canvas; caret and text therefore
//! share one visual midpoint while typing.

use warpui::elements::{
    Align, ChildView, ConstrainedBox, Container, CrossAxisAlignment, DispatchEventResult, Element,
    EventHandler, Expanded, Flex, ParentElement,
};
use warpui::ViewHandle;
use warpui_core::scene::{Border, CornerRadius, Radius};

use crate::icons::{render_icon, AppIcon};
use crate::theme::Theme;
use crate::workspace::{InputEditor, InputEditorAction};

/// Width behaviour of the shared input box.
pub enum SharedInputWidth {
    /// Fixed width in px.
    Fixed(f32),
    /// Fill the available width.
    Fill,
    /// Fill, clamped between min and max px.
    MinMax(f32, f32),
}

/// Leading icon shown inside the box, ahead of the editor.
pub struct SharedInputIcon {
    pub icon: AppIcon,
    pub size: f32,
}

/// All knobs of the shared input box. Anything left as `None` falls back to
/// the reference (settings-search) styling so call sites only specify what
/// differs at their location.
pub struct SharedInputOpts {
    /// Optional leading icon (e.g. search). Rendered in the theme's
    /// disabled-text color unless `icon_color` is set.
    pub leading_icon: Option<SharedInputIcon>,
    /// Optional parent-built leading element, used instead of `leading_icon`
    /// when the site needs a non-icon marker (e.g. a text glyph).
    pub leading_element: Option<Box<dyn Element>>,
    pub icon_color: Option<warpui::color::ColorU>,
    /// Optional trailing element built by the parent (clear button, toggle,
    /// unit label …). The parent owns its hover/click behaviour.
    pub trailing: Option<Box<dyn Element>>,
    pub width: SharedInputWidth,
    /// Fixed height. `None` lets content + vertical padding decide.
    pub height: Option<f32>,
    pub radius: f32,
    pub background: Option<warpui::color::ColorU>,
    pub border: Option<warpui::color::ColorU>,
    pub pad_left: f32,
    pub pad_right: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
    pub spacing: f32,
}

impl Default for SharedInputOpts {
    fn default() -> Self {
        Self {
            leading_icon: None,
            leading_element: None,
            icon_color: None,
            trailing: None,
            width: SharedInputWidth::Fill,
            height: None,
            radius: 7.0,
            background: None,
            border: None,
            pad_left: 8.0,
            pad_right: 8.0,
            pad_top: 6.0,
            pad_bottom: 6.0,
            spacing: 6.0,
        }
    }
}

/// Draw the shared input box around an existing editor view.
pub fn shared_input(
    theme: &Theme,
    editor: &ViewHandle<InputEditor>,
    opts: SharedInputOpts,
) -> Box<dyn Element> {
    let editor_id = editor.id();
    let icon_color = opts.icon_color.unwrap_or_else(|| theme.disabled_text());
    let mut row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    if opts.spacing > 0.0 {
        row = row.with_spacing(opts.spacing);
    }
    if let Some(element) = opts.leading_element {
        row = row.with_child(element);
    } else if let Some(icon) = opts.leading_icon {
        row = row.with_child(render_icon(icon.icon, icon_color, icon.size));
    }
    // The editor canvas expands internally and panics on an unbounded width
    // constraint, so it must always sit in an Expanded slot — never as a bare
    // row child.
    row = row.with_child(Expanded::new(1.0, ChildView::new(editor).finish()).finish());
    if let Some(trailing) = opts.trailing {
        row = row.with_child(trailing);
    }

    // A fixed-height box must loosen the editor's minimum height before
    // centering it. Otherwise its scroll viewport grows to the box height,
    // while the text stays at the viewport's top edge.
    let contents = if opts.height.is_some() {
        Align::new(row.finish()).left().finish()
    } else {
        row.finish()
    };
    let boxed = Container::new(contents)
        .with_background_color(opts.background.unwrap_or_else(|| theme.surface_2()))
        .with_border(
            Border::all(1.0).with_border_color(opts.border.unwrap_or_else(|| theme.border())),
        )
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(opts.radius)))
        .with_padding_left(opts.pad_left)
        .with_padding_right(opts.pad_right)
        .with_padding_top(opts.pad_top)
        .with_padding_bottom(opts.pad_bottom)
        .finish();

    let constrained = ConstrainedBox::new(boxed);
    let constrained = match opts.width {
        SharedInputWidth::Fixed(w) => constrained.with_width(w),
        SharedInputWidth::Fill => constrained,
        SharedInputWidth::MinMax(min, max) => constrained.with_min_width(min).with_max_width(max),
    };
    let constrained = match opts.height {
        Some(h) => constrained.with_height(h),
        None => constrained,
    };
    EventHandler::new(constrained.finish())
        .on_left_mouse_down(move |ctx, _app, position| {
            // The editor handles clicks on its text. Route clicks on the icon,
            // padding, and remaining box surface to the same editor as well.
            ctx.dispatch_typed_action_for(editor_id, InputEditorAction::ClickAt(position));
            DispatchEventResult::StopPropagation
        })
        .finish()
}
