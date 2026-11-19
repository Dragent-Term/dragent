use crate::theme::Theme;
use dragent_pty::PtyFeedStats;
use dragent_terminal::{
    Emulator, HookEvent, MouseEvent, MouseReportingMode, SemanticMarker, TermEvent, TermMode,
};
use std::path::PathBuf;

use super::alt_screen_state::AltScreenState;
use super::block::Block;
use super::block_context::block_context_from_lines;
use super::block_list::BlockList;
use super::events::TerminalEvent;
use super::input_mode::InputMode;
use super::selection::{selection_text, CellSelection};
use super::size::{SizeInfo, SizeReason, SizeUpdate};
use std::sync::Arc;

pub use super::session_core_types::{
    current_rss_mb, ActiveClearGap, SessionEffect, ShellLinkStatus, TerminalSessionCore,
};

#[cfg(test)]
mod tests {
    use super::super::session_core_cwd::{is_default_shell_title, resolve_cd_target};
    use super::{SessionEffect, TerminalEvent, TerminalSessionCore};
    use std::path::{Path, PathBuf};

    fn title_change_count(core: &TerminalSessionCore) -> usize {
        core.pending_effects
            .iter()
            .filter(|effect| matches!(effect, SessionEffect::Ui(TerminalEvent::TitleChanged)))
            .count()
    }

    #[test]
    fn a_session_replaced_on_purpose_leaves_no_modes_failure_or_completion() {
        let theme = crate::theme::Theme::dark();
        let size = crate::terminal::model::SizeInfo {
            cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0,
        };
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, false);
        let mut daemon = dragent_terminal::Emulator::new(24, 80);
        let bytes = b"\x1b]7979;precmd;0;/tmp\x07\x1b]7979;preexec;codex\x07\x1b[?1049h\x1b[?2004h";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert!(core.is_command_running() && core.alt_screen_active());
        core.take_pending_effects();

        core.end_running_command(&theme);

        assert!(!core.is_command_running());
        assert!(!core.alt_screen_active());
        // The next command must not be sent as a paste into the new shell.
        assert!(!core.active_emulator().mode().contains(dragent_terminal::TermMode::BRACKETED_PASTE));
        assert!(!core.take_pending_effects().iter().any(|effect| matches!(
            effect,
            crate::terminal::model::session_core_types::SessionEffect::Ui(
                crate::terminal::model::TerminalEvent::BlockCompleted { .. }
            )
        )));
        let block = core.blocks().blocks().last().expect("the old block is kept");
        assert!(block.completion.as_ref().is_some_and(|done| !done.failed));
    }

    #[test]
    fn an_agent_already_running_when_the_theme_flips_needs_a_restart() {
        let dark = crate::theme::Theme::dark();
        let light = crate::theme::Theme::light();
        let size = crate::terminal::model::SizeInfo {
            cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0,
        };
        let mut daemon = dragent_terminal::Emulator::new(24, 80);
        let run = |command: &str, daemon: &mut dragent_terminal::Emulator| {
            let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, false);
            let bytes = format!("\x1b]7979;precmd;0;/tmp\x07\x1b]7979;preexec;{command}\x07");
            let operations = daemon.feed_recorded(bytes.as_bytes());
            core.feed_operations(bytes.as_bytes(), &operations, &dark);
            core.note_scheme_flipped_from(&dark);
            core
        };
        // It never asked as far as this app saw, so it's on the theme it was showing.
        let agent = run("codex resume abc", &mut daemon);
        assert!(agent.stale_app_colors(&light).is_some_and(|told| told.is_dark));
        assert!(agent.needs_restart_for_theme(&light));
        assert!(!agent.needs_restart_for_theme(&dark), "flipped back: nothing to do");
        // An ordinary command isn't restarted for a theme.
        assert!(!run("npm test", &mut daemon).needs_restart_for_theme(&light));
    }

    #[test]
    fn a_program_that_never_asks_again_keeps_the_scheme_it_was_told() {
        let dark = crate::theme::Theme::dark();
        let light = crate::theme::Theme::light();
        let size = crate::terminal::model::SizeInfo { cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0 };
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, true);
        let mut asked = dragent_terminal::Emulator::new(24, 80);
        let query = b"\x1b]11;?\x1b\\";
        let operations = asked.feed_recorded(query);
        core.feed_operations(query, &operations, &dark);
        // The theme flips. It answered dark and hasn't asked since.
        core.set_reply_theme(light);
        assert!(core.stale_app_colors(&light).is_some_and(|c| c.is_dark));
        assert!(core.stale_app_colors(&dark).is_none());
        assert!(core.needs_restart_for_theme(&light));
        // One that asked to hear about changes follows on its own.
        let wants_updates = b"\x1b[?2031h";
        let updates = asked.feed_recorded(wants_updates);
        core.feed_operations(wants_updates, &updates, &dark);
        assert!(!core.needs_restart_for_theme(&light));
        // Asking again means it has seen the new colours.
        core.feed_operations(query, &operations, &dark);
        assert!(core.stale_app_colors(&light).is_none());
    }

    #[test]
    fn host_resume_restores_agent_routing_and_completion_hook() {
        use super::{Emulator, InputMode, SizeInfo};
        let size = SizeInfo { cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0 };
        let theme = crate::theme::Theme::dark();
        for command in ["claude", "codex", "opencode"] {
            let mut daemon = Emulator::new(24, 80);
            daemon.feed(b"\x1b[?1049h\x1b[?2004hwaiting for input");
            let resume = dragent_host::screen_protocol::TerminalResume {
                display: daemon.display_checkpoint().unwrap(), cwd: "/repo".into(), command: Some(command.into()), shell_ready: true, starting: None, answered_colors: None,
            };
            let mut restored = TerminalSessionCore::new(PathBuf::from("/tmp"), size, true);
            restored.restore_host_resume(&resume, &theme).unwrap();
            assert!(restored.is_command_running());
            assert_eq!(restored.running_command().as_deref(), Some(command));
            assert!(restored.alt_screen_active());
            assert!(!matches!(restored.input_mode, InputMode::Idle));
            assert!(!restored.suppress_startup_output);
            let bytes = b"\x1b[?1049l\x1b]7979;precmd;0;/repo\x07";
            let operations = daemon.feed_recorded(bytes);
            restored.feed_operations(bytes, &operations, &theme);
            assert!(!restored.is_command_running(), "completion must finish the restored block");
        }
    }

    #[test]
    fn host_checkpoint_before_a_submitted_command_keeps_its_single_block() {
        use super::{Emulator, SizeInfo};
        let size = SizeInfo { cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0 };
        let theme = crate::theme::Theme::dark();
        let completions = |core: &TerminalSessionCore| -> Vec<Option<i32>> {
            core.pending_effects
                .iter()
                .filter_map(|effect| match effect {
                    SessionEffect::Ui(TerminalEvent::BlockCompleted { context }) => Some(context.exit_code),
                    _ => None,
                })
                .collect()
        };
        let idle = |daemon: &Emulator| dragent_host::screen_protocol::TerminalResume {
            display: daemon.display_checkpoint().unwrap(), cwd: "/remote".into(), command: None, shell_ready: true, starting: None, answered_colors: None,
        };

        // An installer submitted while its new remote terminal was starting:
        // the checkpoint shows the idle prompt the command is queued behind.
        let mut daemon = Emulator::new(24, 80);
        daemon.feed(b"$ ");
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, true);
        core.begin_command("npm install -g opencode-ai".into());
        core.restore_host_resume(&idle(&daemon), &theme).unwrap();
        assert!(core.is_command_running(), "the queued command's block must stay open");
        assert!(completions(&core).is_empty());
        // The prompt's sentinel fence cell too: Windows completes a block at
        // the fence, since ConPTY interleaves `precmd` into the command's output.
        let bytes = b"\x1b]7979;preexec;npm install -g opencode-ai\x07added 1 package\r\n\x1b]7979;precmd;0;/remote\x07\x1b[48;2;1;2;3m \x1b[49m$ ";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert_eq!(core.blocks().blocks().len(), 1);
        assert_eq!(completions(&core), vec![Some(0)]);

        // A command the shell did run, which ended while detached, still closes.
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, true);
        let bytes = b"\x1b]7979;precmd;0;/remote\x07\x1b]7979;preexec;sleep 5\x07";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert!(core.is_command_running());
        core.restore_host_resume(&idle(&daemon), &theme).unwrap();
        assert!(!core.is_command_running());
    }

    #[test]
    fn remote_interactive_host_resume_keeps_keyboard_with_running_process() {
        use super::{Emulator, InputMode, SizeInfo};
        let size = SizeInfo { cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0 };
        let theme = crate::theme::Theme::dark();
        // Claude's first-run selection screen uses the primary screen, not
        // the alternate screen: input routing must use command state as well.
        let mut daemon = Emulator::new(24, 80);
        daemon.feed(b"\x1b[?2004hChoose a theme\r\n> Dark mode");
        let mut resume = dragent_host::screen_protocol::TerminalResume {
            display: daemon.display_checkpoint().unwrap(), cwd: "/remote/project".into(),
            command: Some("claude --dangerously-skip-permissions 'hey'".into()), shell_ready: true,
            starting: None, answered_colors: None,
        };
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, true);
        core.remote_machine = Some("test-machine".into());
        core.restore_host_resume(&resume, &theme).unwrap();
        assert!(!core.alt_screen_active());
        assert!(core.is_command_running());
        assert!(!matches!(core.input_mode, InputMode::Idle));
        assert!(core.blocks().blocks()[0].remote_filesystem);

        // Old runtimes report neither readiness nor a running command. They
        // must not mark an interactive process completed and expose our editor.
        resume.shell_ready = false;
        resume.command = None;
        assert!(core.restore_host_resume(&resume, &theme).unwrap_err().contains("Shell integration"));
        assert!(core.is_command_running(), "failed restore must leave routing intact");

        let bytes = b"\x1b]7979;precmd;130;/remote/project\x07";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert!(!core.is_command_running());
        assert!(matches!(core.input_mode, InputMode::Idle));
    }

    #[test]
    fn a_starting_host_shell_keeps_its_queued_command_through_the_first_prompt() {
        use super::{Emulator, SizeInfo};
        let size = SizeInfo { cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0 };
        let theme = crate::theme::Theme::dark();
        let shell = r"C:\Program Files\Git\bin\bash.exe";
        let mut daemon = Emulator::new(24, 80);
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, true);
        // The pane's launch command is submitted before the slow shell is up;
        // the daemon holds it until the shell's first prompt.
        core.begin_command("claude".into());
        let resume = dragent_host::screen_protocol::TerminalResume {
            display: daemon.display_checkpoint().unwrap(), cwd: "/repo".into(), command: None,
            shell_ready: false, starting: Some(shell.into()), answered_colors: None,
        };
        core.restore_host_resume(&resume, &theme)
            .expect("a starting shell is not an unintegrated one");
        assert_eq!(core.host_shell_starting(), Some(shell));
        assert!(core.is_command_running());

        // The shell starting: its prompt and the typed integration echo.
        let bytes = b"$ export PATH=startup-noise";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        let live = |core: &TerminalSessionCore| {
            core.blocks().blocks()[0].emulator.as_ref().unwrap().cursor_line_text()
        };
        assert!(live(&core).contains("startup-noise"));

        // The startup prompt ends "starting" but not the queued command, and
        // the startup output is not shown as that command's output.
        let bytes = b"\x1b]7979;precmd;0;/repo\x07";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert_eq!(core.host_shell_starting(), None);
        assert!(core.is_command_running(), "the queued command must survive the startup prompt");
        assert!(!live(&core).contains("startup-noise"), "startup output leaked into the block");

        // The held command then runs and completes as usual.
        let bytes = b"\x1b]7979;preexec;claude\x07done\r\n\x1b]7979;precmd;0;/repo\x07";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert!(!core.is_command_running());
        assert_eq!(core.blocks().blocks().len(), 1, "one block for the launch command");
    }

    #[test]
    fn host_pane_resizes_its_process_only_when_the_restored_layout_differs() {
        use super::{Emulator, SizeInfo, SizeReason, SizeUpdate};
        let size = |rows: u16, cols: u16| SizeInfo {
            cols,
            rows,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: cols as f32 * 8.0,
            viewport_height_px: rows as f32 * 14.0,
        };
        let theme = crate::theme::Theme::dark();
        let resume = |rows: usize, cols: usize, command: Option<&str>| {
            let mut daemon = Emulator::new(rows, cols);
            if command.is_some() {
                daemon.feed(b"\x1b[?1049h\x1b[?2004hwaiting for input");
            }
            dragent_host::screen_protocol::TerminalResume {
                display: daemon.display_checkpoint().unwrap(),
                cwd: "/repo".into(),
                command: command.map(str::to_string),
                shell_ready: true,
                starting: None, answered_colors: None,
            }
        };

        // Restored agent: the pane's first layout happens before it knows an
        // agent is running (idle input bar), so it must not reach the PTY.
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size(24, 80), true);
        core.set_host_resume_pending(true);
        let mut sent = Vec::new();
        core.apply_size_update(SizeUpdate::new(size(40, 120), SizeReason::WindowResize), |r, c| {
            sent.push((r, c))
        });
        core.restore_host_resume(&resume(45, 120, Some("claude")), &theme).unwrap();
        assert_eq!((core.size.rows, core.size.cols), (45, 120));
        // A measurement taken before the view saw the restore is stale.
        core.apply_size_update(SizeUpdate::new(size(40, 120), SizeReason::WindowResize), |r, c| {
            sent.push((r, c))
        });
        assert_eq!((core.size.rows, core.size.cols), (45, 120));
        core.acknowledge_host_restore();
        core.apply_size_update(SizeUpdate::new(size(45, 120), SizeReason::WindowResize), |r, c| {
            sent.push((r, c))
        });
        assert!(sent.is_empty(), "an unchanged layout must not resize the agent: {sent:?}");
        core.apply_size_update(SizeUpdate::new(size(44, 120), SizeReason::WindowResize), |r, c| {
            sent.push((r, c))
        });
        assert_eq!(sent, vec![(44, 120)]);

        // New session: the daemon starts it at 24×80, so the pane's real size
        // is applied once the checkpoint arrives.
        let mut fresh = TerminalSessionCore::new(PathBuf::from("/tmp"), size(24, 80), true);
        fresh.set_host_resume_pending(true);
        let mut sent = Vec::new();
        fresh.apply_size_update(SizeUpdate::new(size(40, 120), SizeReason::WindowResize), |r, c| {
            sent.push((r, c))
        });
        fresh.restore_host_resume(&resume(24, 80, None), &theme).unwrap();
        fresh.acknowledge_host_restore();
        fresh.apply_size_update(SizeUpdate::new(size(40, 120), SizeReason::WindowResize), |r, c| {
            sent.push((r, c))
        });
        assert_eq!(sent, vec![(40, 120)]);
    }

    #[test]
    fn host_hook_boundary_routes_following_output_into_agent_block() {
        use super::{Emulator, SizeInfo};
        let size = SizeInfo { cols: 80, rows: 24, cell_width: 8.0, cell_height: 14.0,
            viewport_width_px: 640.0, viewport_height_px: 336.0 };
        let theme = crate::theme::Theme::dark();
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, false);
        let mut daemon = Emulator::new(24, 80);
        let bytes = b"\x1b]7979;precmd;0;/tmp\x07\x1b]7979;preexec;claude\x07\x1b[?1049hagent output";
        let operations = daemon.feed_recorded(bytes);
        core.feed_operations(bytes, &operations, &theme);
        assert_eq!(core.running_command().as_deref(), Some("claude"));
        assert!(core.active_emulator().alt_screen_active());
        assert!(!core.session.alt_screen_active(), "agent mode must not contaminate the shell emulator");
        assert!(core.active_emulator().alt_screen_lines()[0].plain_text().contains("agent output"));
    }

    #[test]
    fn ai_repl_window_resize_is_forwarded_to_the_pty() {
        use super::{SizeInfo, SizeReason, SizeUpdate};

        let initial = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), initial, false);
        core.begin_command("codex".to_string());

        let resized = SizeInfo {
            rows: 18,
            viewport_height_px: 252.0,
            ..initial
        };
        let mut signaled = None;
        core.apply_size_update(
            SizeUpdate::new(resized, SizeReason::WindowResize),
            |rows, cols| signaled = Some((rows, cols)),
        );

        assert_eq!(signaled, Some((18, 80)));
        assert_eq!(core.size().rows, 18);
    }

    #[test]
    fn repeated_identical_window_title_emits_one_title_change() {
        use super::SizeInfo;

        let size = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let theme = crate::theme::Theme::dark();
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, false);

        core.feed_pty_bytes(b"\x1b]0;npm run dev\x07", &theme);
        assert_eq!(title_change_count(&core), 1);

        core.feed_pty_bytes(b"\x1b]0;npm run dev\x07", &theme);
        assert_eq!(
            title_change_count(&core),
            1,
            "an unchanged OSC title must not retrigger cwd/git synchronization"
        );

        core.feed_pty_bytes(b"\x1b]0;vite\x07", &theme);
        assert_eq!(title_change_count(&core), 2);
    }

    #[cfg(windows)]
    #[test]
    fn msys_drive_paths_normalize_to_native() {
        use super::super::session_core_cwd::normalize_shell_cwd;
        assert_eq!(
            normalize_shell_cwd(PathBuf::from("/c/Users/x/dev")),
            PathBuf::from("C:\\Users\\x\\dev")
        );
        assert_eq!(
            normalize_shell_cwd(PathBuf::from("/d")),
            PathBuf::from("D:\\")
        );
        // Native and MSYS-root paths pass through untouched.
        assert_eq!(
            normalize_shell_cwd(PathBuf::from("C:\\Users\\x")),
            PathBuf::from("C:\\Users\\x")
        );
        assert_eq!(
            normalize_shell_cwd(PathBuf::from("/usr/bin")),
            PathBuf::from("/usr/bin")
        );
    }

    #[test]
    fn default_shell_titles_are_filtered() {
        // conhost / elevated conhost initial titles
        assert!(is_default_shell_title(
            "Administrator: C:\\WINDOWS\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        ));
        assert!(is_default_shell_title(
            "C:\\Program Files\\Git\\bin\\bash.exe"
        ));
        assert!(is_default_shell_title("pwsh.exe"));
        // MSYS2 / Git Bash cd titles
        assert!(is_default_shell_title("CLANGARM64:/c/Users/x/dev/dragent"));
        assert!(is_default_shell_title("MINGW64:~"));
        assert!(is_default_shell_title("MSYS:/usr/bin"));
        // product-name titles
        assert!(is_default_shell_title("Windows PowerShell"));
        assert!(is_default_shell_title("PowerShell 7 (arm64)"));
        // bare paths
        assert!(is_default_shell_title("C:\\Users\\x\\dev"));
        assert!(is_default_shell_title("/usr/local/bin"));
        assert!(is_default_shell_title("~/dev/dragent"));

        // Linux default bash/zsh titles
        assert!(is_default_shell_title("user@ubuntu: ~/dev"));
        assert!(is_default_shell_title("jahsminemma@web-01: /var/log"));

        // deliberate app titles must survive
        assert!(!is_default_shell_title("✳ Claude Code"));
        assert!(!is_default_shell_title("claude — arm-windows"));
        assert!(!is_default_shell_title("vim README.md"));
        assert!(!is_default_shell_title("npm run dev"));
        assert!(!is_default_shell_title("My Project — build running"));
    }

    #[test]
    fn resolve_cd_parent_directory() {
        let current = Path::new("/Users/me/project");
        assert_eq!(
            resolve_cd_target(current, "cd .."),
            Some(PathBuf::from("/Users/me"))
        );
    }

    #[test]
    fn resolve_cd_chained_command() {
        let current = Path::new("/repo/pkg");
        assert_eq!(
            resolve_cd_target(current, "cd .. && ls"),
            Some(PathBuf::from("/repo"))
        );
    }

    #[test]
    fn resolve_cd_quoted_parent() {
        let current = Path::new("/Users/me/project");
        assert_eq!(
            resolve_cd_target(current, "cd \"..\""),
            Some(PathBuf::from("/Users/me"))
        );
    }

    #[test]
    fn alt_screen_enter_on_1049() {
        let mut emu = dragent_terminal::Emulator::new(24, 80);
        emu.feed(b"\x1b[?1049h");
        assert!(emu.alt_screen_active());
    }

    /// Windows ConPTY interleaves the `precmd` OSC into the middle of a
    /// command's rendered output. The drain path must keep capturing until the
    /// rendered sentinel-cell fence (SGR bg 48;2;1;2;3 — see
    /// `HookEvent::Fence`) so output arriving after the OSC still lands in the
    /// block — and the prompt after the fence must not.
    #[test]
    #[cfg(windows)]
    fn drain_captures_output_arriving_after_interleaved_precmd() {
        use crate::terminal::model::size::SizeInfo;
        let size = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let theme = crate::theme::Theme::dark();
        let mut core = super::TerminalSessionCore::new(PathBuf::from("/"), size, false);

        core.begin_command("ls".to_string());
        assert!(core.is_command_running());

        // Echo of the submitted command (line 0, dropped at snapshot).
        core.feed_pty_bytes(b"ls\r\n", &theme);
        // ConPTY splits mid-line: first entry, then the precmd OSC, in one chunk.
        core.feed_pty_bytes(b"backend\x1b]7979;precmd;0;C:/tmp\x1b\\/  ", &theme);

        // precmd returns the input bar, but the block must still be capturing.
        assert!(!core.is_command_running());
        let block = &core.blocks().blocks()[0];
        assert!(block.is_completed());
        assert!(
            block.emulator.is_some(),
            "block should keep capturing while draining"
        );

        // Remaining output, the rendered sentinel-cell fence, then prompt bytes.
        core.feed_pty_bytes(
            b"frontend/  README.md\r\n\x1b[48;2;1;2;3m \x1b[49mPS>",
            &theme,
        );

        let block = &core.blocks().blocks()[0];
        assert!(block.emulator.is_none(), "fence should finalise the drain");
        let text: String = block
            .lines
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("backend"), "missing first entry: {text:?}");
        assert!(
            text.contains("frontend"),
            "output after precmd OSC lost: {text:?}"
        );
        assert!(text.contains("README.md"), "tail lost: {text:?}");
        assert!(!text.contains("PS>"), "prompt leaked into block: {text:?}");
    }

    /// Fast commands (`cd`) deliver precmd AND the fence in one chunk, before
    /// any drain state exists. The fence scan must run for the ACTIVE block
    /// too, split there, and finalise without the 160ms timer — and without
    /// the prompt leaking into the block.
    #[test]
    #[cfg(windows)]
    fn same_chunk_precmd_and_fence_finalises_immediately() {
        use crate::terminal::model::size::SizeInfo;
        let size = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let theme = crate::theme::Theme::dark();
        let mut core = super::TerminalSessionCore::new(PathBuf::from("/"), size, false);

        core.begin_command("cd dev".to_string());
        core.feed_pty_bytes(
            b"cd dev\r\n\x1b]7979;precmd;0;C:/tmp/dev\x1b\\\x1b[48;2;1;2;3m \x1b[49mPS C:/tmp/dev>",
            &theme,
        );

        let block = &core.blocks().blocks()[0];
        assert!(block.is_completed());
        assert!(
            block.emulator.is_none(),
            "same-chunk fence should finalise without waiting for the timer"
        );
        let text: String = block
            .lines
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains("PS C:/tmp/dev>"),
            "prompt leaked into block: {text:?}"
        );
    }

    /// Replay a real `npm install` ConPTY capture (recorded by the
    /// dragent_pty `npm_repro` harness with the exact chunk boundaries the
    /// PTY delivered) through the session core, and assert the block
    /// finalises. Reproduces the "npm install never completes" bug.
    ///
    /// Run:
    ///   DRAGENT_NPM_CAPTURE=<file> cargo test -p dragent replay_npm_capture -- --nocapture --ignored
    #[test]
    #[cfg(windows)]
    #[ignore = "needs a capture file from the npm_repro harness"]
    fn replay_npm_capture_completes_block() {
        use crate::terminal::model::size::SizeInfo;
        let path = std::env::var("DRAGENT_NPM_CAPTURE").expect("set DRAGENT_NPM_CAPTURE");
        let data = std::fs::read(&path).expect("read capture");
        let mut chunks = Vec::new();
        let mut i = 0usize;
        while i + 4 <= data.len() {
            let len = u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
            i += 4;
            chunks.push(&data[i..i + len]);
            i += len;
        }
        println!("replaying {} chunks", chunks.len());

        let size = SizeInfo {
            cols: 120,
            rows: 30,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 960.0,
            viewport_height_px: 420.0,
        };
        let theme = crate::theme::Theme::dark();
        let mut core = super::TerminalSessionCore::new(PathBuf::from("C:/tmp"), size, false);

        core.begin_command("npm install --no-audit --no-fund".to_string());
        assert!(core.is_command_running());

        for (n, chunk) in chunks.iter().enumerate() {
            core.feed_pty_bytes(chunk, &theme);
            let block = &core.blocks().blocks()[0];
            println!(
                "chunk {n}: {}B running={} completed={} live={}",
                chunk.len(),
                core.is_command_running(),
                block.is_completed(),
                block.emulator.is_some(),
            );
        }

        let block = &core.blocks().blocks()[0];
        let text: String = block
            .lines
            .iter()
            .map(|l| l.plain_text())
            .collect::<Vec<_>>()
            .join("\n");
        println!("---- block text ----\n{text}\n----");
        assert!(block.is_completed(), "precmd never reached the block");
        assert!(
            block.emulator.is_none(),
            "fence never finalised the drain (block still live)"
        );
        assert!(
            text.contains("added"),
            "npm summary line missing from block: {text:?}"
        );
    }

    #[test]
    fn test_clear_viewport_and_gap_shrinking() {
        use crate::terminal::model::size::SizeInfo;
        let size = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let mut core = super::TerminalSessionCore::new(PathBuf::from("/"), size, false);

        // Initially no clear gap
        assert!(core.active_clear_gap().is_none());

        // Apply clear viewport
        core.apply_clear_viewport(300.0);

        let gap = core
            .active_clear_gap()
            .expect("Should have active clear gap");
        assert_eq!(gap.original_height_px, 300.0);
        assert_eq!(gap.current_height_px, 300.0);

        // Shrink the clear gap
        let mut block_heights = std::collections::HashMap::new();
        // Add a block before or after?
        // Since gap.before_block_id is None (we had no blocks when we cleared), all blocks are considered "after"
        block_heights.insert(1, 100.0);

        let empty_agent_heights = std::collections::HashMap::new();
        let empty_shell_created = std::collections::HashMap::new();
        let empty_agent_created = std::collections::HashMap::new();

        core.shrink_active_clear_gap(
            &block_heights,
            &empty_agent_heights,
            &empty_shell_created,
            &empty_agent_created,
            40.0,
        );

        let gap2 = core.active_clear_gap().expect("Gap should still be active");
        // height_added = 100.0 (block 1) + 24.0 (has_blocks_after spacing) + 40.0 (live_prompt) = 164.0
        // current_height = 300.0 - 164.0 = 136.0
        assert!((gap2.current_height_px - 136.0).abs() < 0.1);

        // Shrink more until it disappears
        block_heights.insert(2, 200.0);
        core.shrink_active_clear_gap(
            &block_heights,
            &empty_agent_heights,
            &empty_shell_created,
            &empty_agent_created,
            40.0,
        );
        assert!(
            core.active_clear_gap().is_none(),
            "Gap should disappear when shrunk past 0"
        );
    }

    #[test]
    fn clear_command_block_is_marked_as_the_gap_trigger() {
        // Regression test: running `clear` after some history used to leave
        // the `clear` command's own (empty) block rendering at the bottom,
        // below the gap, and left a stale sticky header from before the
        // clear pinned at the top. `apply_clear_viewport` must record which
        // block IS the `clear` command so the render layer can exclude it.
        use crate::terminal::model::size::SizeInfo;
        let size = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let theme = crate::theme::Theme::dark();
        let mut core = super::TerminalSessionCore::new(PathBuf::from("/"), size, false);

        // A prior, unrelated completed command.
        core.on_preexec("ls".to_string());
        let ls_id = core.active_block_id.expect("ls block should be active");
        core.on_precmd(Some(0), None, &theme);

        // The user then runs `clear` — it becomes the new active block.
        core.on_preexec("clear".to_string());
        let clear_id = core.active_block_id.expect("clear block should be active");
        assert_ne!(ls_id, clear_id);

        // The Clear hook fires DURING the command's own execution, before precmd.
        core.apply_clear_viewport(300.0);

        let gap = core
            .active_clear_gap()
            .expect("clear must produce an active gap");
        assert_eq!(
            gap.before_block_id,
            Some(ls_id),
            "gap anchors right after the prior command"
        );
        assert_eq!(
            gap.trigger_block_id,
            Some(clear_id),
            "the clear command's own block must be identified for exclusion from rendering"
        );
    }

    /// THROWAWAY repro: feed a captured daemon ring through the parser in
    /// poll-sized slices and dump block text. Delete after diagnosing.
    #[test]
    #[ignore = "needs a captured ring at /tmp/full-term-763.bin"]
    fn throwaway_ring_repro() {
        use super::SizeInfo;
        let raw = std::fs::read("/tmp/full-term-763.bin").expect("ring fixture");
        let size = SizeInfo {
            cols: 80,
            rows: 24,
            cell_width: 8.0,
            cell_height: 14.0,
            viewport_width_px: 640.0,
            viewport_height_px: 336.0,
        };
        let theme = crate::theme::Theme::dark();
        let mut core = TerminalSessionCore::new(PathBuf::from("/tmp"), size, false);
        // Simulate 500ms poll delivery in odd slices (unaligned splits).
        let mut off = 0;
        let mut lens = vec![];
        while off < raw.len() {
            let n = 3000 + (off % 4096);
            let end = (off + n).min(raw.len());
            core.feed_pty_bytes(&raw[off..end], &theme);
            lens.push(end - off);
            off = end;
        }
        eprintln!("fed {} chunks", lens.len());
        let mut hits = 0;
        for block in core.blocks().blocks().iter() {
            for line in block.lines.iter() {
                let text = line.plain_text();
                if text.contains(";48;") || text.contains(";34;") {
                    hits += 1;
                    if hits <= 3 {
                        eprintln!("GARBAGE LINE: {text:?}");
                    }
                }
            }
        }
        eprintln!(
            "blocks={} garbage_lines={hits}",
            core.blocks().blocks().len()
        );
        assert_eq!(hits, 0, "raw escape params must never render as text");
    }
}
