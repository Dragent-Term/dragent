use crate::{AppContext, Effect, EntityId, WindowId};

/// An owned, cloneable token representing a view's focus identity.
///
/// Unlike [`ViewContext::is_self_focused`], which requires being inside a
/// `ViewContext`, a `FocusHandle` bundles the view's identity and can be
/// queried directly inside [`View::render`], which only receives an immutable
/// `&AppContext`.
///
/// Create one with [`ViewContext::focus_handle`] during view construction and
/// store it in the view struct.  It is cheap to clone (two integer copies).
///
/// # Example
///
/// ```ignore
/// struct MyView {
///     focus_handle: FocusHandle,
/// }
///
/// impl View for MyView {
///     fn render(&self, cx: &AppContext) -> Box<dyn Element> {
///         let border = if self.focus_handle.is_focused(cx) {
///             cx.theme().accent
///         } else {
///             cx.theme().border
///         };
///         // ...
///     }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FocusHandle {
    pub(in crate::core) view_id: EntityId,
    pub(in crate::core) window_id: WindowId,
}

impl FocusHandle {
    /// Returns `true` if this view currently holds keyboard focus.
    pub fn is_focused(&self, cx: &AppContext) -> bool {
        cx.focused_view_id(self.window_id) == Some(self.view_id)
    }

    /// Returns `true` if this view **or any of its descendants** currently holds focus.
    pub fn is_focused_within(&self, cx: &AppContext) -> bool {
        cx.check_view_or_child_focused(self.window_id, &self.view_id)
    }

    /// Queues a focus request for this view.
    ///
    /// Equivalent to [`ViewContext::focus_self`] but callable from any code
    /// that holds the handle.  The focus change is deferred as an
    /// `Effect::Focus` and applied during the next effect flush, exactly as
    /// `ctx.focus_self()` would be.
    pub fn request_focus(&self, cx: &mut AppContext) {
        cx.pending_effects.push_back(Effect::Focus {
            window_id: self.window_id,
            view_id: self.view_id,
        });
    }
}
