//! Copy buttons and the block options menu, through the real terminal view.

use super::actions::{block_copy_key, TerminalAction};
use super::copy_confirmation::COPY_CONFIRMATION;
use super::TerminalView;
use crate::realtime::manager::SessionManager;
use std::sync::Arc;
use std::time::Duration;
use warpui::platform::WindowStyle;
use warpui::r#async::Timer;
use warpui::{App, ReadView, TypedActionView, UpdateView, ViewHandle};

fn with_terminal<F>(test: impl FnOnce(App, ViewHandle<TerminalView>) -> F + 'static)
where
    F: std::future::Future<Output = ()> + 'static,
{
    App::test(dragent_assets::DragentAssets, |mut app| async move {
        let directory = tempfile::tempdir().unwrap();
        app.update(|ctx| {
            ctx.add_singleton_model(crate::auth::AuthManager::preview);
            ctx.add_singleton_model(crate::settings::AccountStore::new);
            ctx.add_singleton_model(|_| SessionManager {
                session: None,
                forwards: Vec::new(),
                forward_error: None,
                forward_busy: false,
                #[cfg(not(target_family = "wasm"))]
                _forward_tunnel: None,
                idle_epoch: 0,
                share_busy: None,
                viewer_theme_id: None,
            });
        });
        let path = directory.path().to_path_buf();
        let (_, terminal) = app.add_window(WindowStyle::NotStealFocus, move |ctx| {
            TerminalView::new_remote(path, Arc::new(|_| {}), ctx)
        });
        test(app, terminal).await;
    });
}

fn act(app: &mut App, terminal: &ViewHandle<TerminalView>, action: TerminalAction) {
    terminal.update(app, |view, ctx| view.handle_action(&action, ctx));
}

fn copied_key(app: &App, terminal: &ViewHandle<TerminalView>) -> Option<String> {
    terminal.read(app, |view, _| {
        view.copy_confirmation.key().map(str::to_string)
    })
}

fn copy_block_command(block_id: usize, text: &str) -> TerminalAction {
    TerminalAction::CopyWithConfirmation {
        key: block_copy_key(block_id),
        text: text.to_string(),
    }
}

#[test]
fn copying_shows_the_confirmation_then_it_goes_away() {
    with_terminal(|mut app, terminal| async move {
        act(&mut app, &terminal, copy_block_command(7, "cargo test"));
        assert_eq!(copied_key(&app, &terminal), Some(block_copy_key(7)));
        let clipboard = app.update(|ctx| ctx.clipboard().read());
        assert_eq!(clipboard.plain_text, "cargo test");

        Timer::after(COPY_CONFIRMATION + Duration::from_millis(200)).await;
        assert_eq!(copied_key(&app, &terminal), None);
    });
}

#[test]
fn copying_nothing_confirms_nothing() {
    with_terminal(|mut app, terminal| async move {
        act(&mut app, &terminal, copy_block_command(7, ""));
        assert_eq!(copied_key(&app, &terminal), None);
    });
}

#[test]
fn every_menu_copy_option_closes_the_menu() {
    with_terminal(|mut app, terminal| async move {
        for action in [
            copy_block_command(7, "cargo test"),
            // No such block, so there is nothing to copy; the menu still closes.
            TerminalAction::CopyBlockOutput { block_id: 7 },
            TerminalAction::CopyBlockCommandAndOutput { block_id: 7 },
        ] {
            terminal.update(&mut app, |view, _| view.open_block_menu = Some(7));
            act(&mut app, &terminal, action);
            assert_eq!(terminal.read(&app, |view, _| view.open_block_menu), None);
        }
    });
}
