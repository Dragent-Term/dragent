//! Icon registry — Lucide SVGs served by `dragent_assets`.
//!
//! Icons live in `app/assets/dragent/icons/` (ISC license). Rendered via
//! `warpui::elements::Icon` + `DragentAssets` as the `AssetProvider`.

use warpui::color::ColorU;
use warpui::elements::{ConstrainedBox, Element, Icon};

pub const ICON_SIZE_SM: f32 = 12.0;
pub const ICON_SIZE_MD: f32 = 16.0;

/// Semantic icon identifiers used in the Dragent UI.
#[derive(Clone, Copy, Debug)]
pub enum AppIcon {
    /// A themed file/language asset whose path is supplied by the icon registry.
    Asset(&'static str),
    Search,
    Plus,
    XClose,
    LayoutLeft,
    LayoutRight,
    LayoutBottom,
    SplitVertical,
    SplitHorizontal,
    Tool,
    Grid,
    Inbox,
    DotsVertical,
    ArrowLeft,
    ArrowRight,
    Terminal,
    TerminalPanel,
    Share,
    Copy,
    Save,
    Edit,
    Replace,
    ReplaceAll,
    ArrowUp,
    ArrowDown,
    Folder,
    FolderOpen,
    GitBranch,
    ClaudeLogo,
    GeminiLogo,
    AmpLogo,
    CopilotLogo,
    OpenCodeLogo,
    Monitor,
    Smartphone,
    Maximize2,
    QrCode,
    Sparkle,
    Star,
    /// Marks a coordination's lead.
    Crown,
    Microphone,
    MessageSquare,
    /// Agent mode. Distinct from [`AppIcon::MessageSquare`] on purpose: a speech
    /// bubble means "a message", which is what every chat app in the dock looks
    /// like — it does not say that something acts on your behalf.
    Agent,
    ChatgptLogo,
    OllamaLogo,
    Sliders,
    Settings,
    Code,
    File,
    User,
    Key,
    Server,
    /// Language-server health and management.
    Bolt,
    Palette,
    Keyboard,
    Eye,
    Info,
    ChevronUp,
    ChevronDown,
    /// Window caption controls (custom titlebar on Windows). Drawn to fill the
    /// viewBox with hairline strokes so a 16px render yields the native 10px
    /// glyph — do not reuse for regular toolbar icons.
    Minimize,
    Maximize,
    Restore,
    PanelMaximize,
    PanelRestore,
    CaptionClose,
    /// Discard-changes / revert-file glyph — same curved-undo-arrow shape as
    /// VS Code's and Cursor's "Discard Changes" action in Source Control.
    Undo2,
    Check,
    ChevronRight,
    CornerDownLeft,
    /// Session history browser trigger — clock-with-a-backward-arrow glyph.
    History,
    /// Padlock — shown on the disabled input of a view-only shared session.
    Lock,
    /// Sidebar navigation: workspaces, automations, tasks, pull requests.
    Layers,
    Clock,
    TaskList,
    PullRequest,
    Sort,
    GitCompare,
    GitHub,
    /// Remote machines (Settings → Machines, Add Project location).
    Activity,
    Ellipsis,
    Plug,
    Download,
    Play,
    Home,
    Trash,
    CircleCheck,
    CircleX,
    FolderPlus,
    ChevronLeft,
    RefreshCw,
    FilePlus,
    FolderGit,
    Power,
    RotateCcw,
    Cpu,
    MemoryStick,
    HardDrive,
    TriangleAlert,
    /// Browser panes.
    Globe,
}

impl AppIcon {
    pub fn path(self) -> &'static str {
        match self {
            AppIcon::Asset(path) => path,
            AppIcon::Star => "icons/star.svg",
            AppIcon::Crown => "icons/crown.svg",
            AppIcon::Search => "icons/search.svg",
            AppIcon::Plus => "icons/plus.svg",
            AppIcon::XClose => "icons/x-close.svg",
            AppIcon::LayoutLeft => "icons/layout-left.svg",
            AppIcon::LayoutRight => "icons/layout-right.svg",
            AppIcon::LayoutBottom => "icons/layout-bottom.svg",
            AppIcon::SplitVertical => "icons/split-vertical.svg",
            AppIcon::SplitHorizontal => "icons/split-horizontal.svg",
            AppIcon::Tool => "icons/tool.svg",
            AppIcon::Grid => "icons/grid.svg",
            AppIcon::Inbox => "icons/inbox.svg",
            AppIcon::DotsVertical => "icons/dots-vertical.svg",
            AppIcon::ArrowLeft => "icons/arrow-left.svg",
            AppIcon::ArrowRight => "icons/arrow-right.svg",
            AppIcon::Terminal => "icons/terminal.svg",
            AppIcon::TerminalPanel => "icons/terminal-panel.svg",
            AppIcon::Share => "icons/share.svg",
            AppIcon::Copy => "icons/copy.svg",
            AppIcon::Save => "icons/save.svg",
            AppIcon::Edit => "icons/edit.svg",
            AppIcon::Replace => "icons/replace.svg",
            AppIcon::ReplaceAll => "icons/replace-all.svg",
            AppIcon::ArrowUp => "icons/arrow-up.svg",
            AppIcon::ArrowDown => "icons/arrow-down.svg",
            AppIcon::Folder => "icons/folder.svg",
            AppIcon::FolderOpen => "icons/folder-open.svg",
            AppIcon::GitBranch => "icons/git-branch.svg",
            AppIcon::ClaudeLogo => "icons/claude-logo.svg",
            AppIcon::GeminiLogo => "icons/gemini-logo.svg",
            AppIcon::AmpLogo => "icons/amp-logo.svg",
            AppIcon::CopilotLogo => "icons/copilot-logo.svg",
            AppIcon::OpenCodeLogo => "icons/opencode-logo.svg",
            AppIcon::Monitor => "icons/monitor.svg",
            AppIcon::Smartphone => "icons/smartphone.svg",
            AppIcon::Maximize2 => "icons/maximize-2.svg",
            AppIcon::QrCode => "icons/qr-code.svg",
            AppIcon::Sparkle => "icons/sparkle.svg",
            AppIcon::Microphone => "icons/microphone.svg",
            AppIcon::MessageSquare => "icons/message-square.svg",
            AppIcon::Agent => "icons/ai-assistant.svg",
            AppIcon::ChatgptLogo => "icons/chatgpt-logo.svg",
            AppIcon::OllamaLogo => "icons/ollama-logo.svg",
            AppIcon::Sliders => "icons/sliders.svg",
            AppIcon::Settings => "icons/settings.svg",
            AppIcon::Code => "icons/code.svg",
            AppIcon::File => "icons/file.svg",
            AppIcon::User => "icons/user.svg",
            AppIcon::Key => "icons/key.svg",
            AppIcon::Server => "icons/server.svg",
            AppIcon::Activity => "icons/activity.svg",
            AppIcon::Ellipsis => "icons/ellipsis.svg",
            AppIcon::Plug => "icons/plug.svg",
            AppIcon::Download => "icons/download.svg",
            AppIcon::Play => "icons/play.svg",
            AppIcon::Home => "icons/house.svg",
            AppIcon::Trash => "icons/trash-2.svg",
            AppIcon::CircleCheck => "icons/circle-check.svg",
            AppIcon::CircleX => "icons/circle-x.svg",
            AppIcon::FolderPlus => "icons/folder-plus.svg",
            AppIcon::ChevronLeft => "icons/chevron-left.svg",
            AppIcon::RefreshCw => "icons/refresh-cw.svg",
            AppIcon::FilePlus => "icons/file-plus.svg",
            AppIcon::FolderGit => "icons/folder-git-2.svg",
            AppIcon::Power => "icons/power.svg",
            AppIcon::RotateCcw => "icons/rotate-ccw.svg",
            AppIcon::Cpu => "icons/cpu.svg",
            AppIcon::MemoryStick => "icons/memory-stick.svg",
            AppIcon::HardDrive => "icons/hard-drive.svg",
            AppIcon::TriangleAlert => "icons/triangle-alert.svg",
            AppIcon::Globe => "icons/globe.svg",
            AppIcon::Bolt => "icons/bolt.svg",
            AppIcon::Palette => "icons/palette.svg",
            AppIcon::Keyboard => "icons/keyboard.svg",
            AppIcon::Eye => "icons/eye.svg",
            AppIcon::Info => "icons/info.svg",
            AppIcon::ChevronUp => "icons/chevron-up.svg",
            AppIcon::ChevronDown => "icons/chevron-down.svg",
            AppIcon::Minimize => "icons/minimize.svg",
            AppIcon::Maximize => "icons/maximize.svg",
            AppIcon::Restore => "icons/restore.svg",
            AppIcon::PanelMaximize => "icons/panel-maximize.svg",
            AppIcon::PanelRestore => "icons/panel-restore.svg",
            AppIcon::CaptionClose => "icons/caption-close.svg",
            AppIcon::Undo2 => "icons/undo-2.svg",
            AppIcon::Check => "icons/check.svg",
            AppIcon::ChevronRight => "icons/chevron-right.svg",
            AppIcon::CornerDownLeft => "icons/corner-down-left.svg",
            AppIcon::History => "icons/history.svg",
            AppIcon::Lock => "icons/lock.svg",
            AppIcon::Layers => "icons/layers.svg",
            AppIcon::Clock => "icons/clock.svg",
            AppIcon::TaskList => "icons/clipboard-list.svg",
            AppIcon::PullRequest => "icons/git-pull-request.svg",
            AppIcon::Sort => "icons/sort.svg",
            AppIcon::GitCompare => "icons/git-compare.svg",
            AppIcon::GitHub => "icons/github.svg",
        }
    }
}

/// Renders a monochrome SVG icon at the given size.
pub fn render_icon(icon: AppIcon, color: ColorU, size: f32) -> Box<dyn Element> {
    ConstrainedBox::new(Icon::new(icon.path(), color).finish())
        .with_width(size)
        .with_height(size)
        .finish()
}

/// Frames in the indeterminate spinner. `Icon` takes a `&'static str` asset
/// path and has no rotation transform, so the spin is pre-baked: each frame is
/// the same arc drawn from a start angle 30° further round.
pub const SPINNER_FRAMES: usize = 12;

/// One spinner frame; `frame` wraps, so callers can just increment a counter.
pub fn render_spinner(frame: usize, color: ColorU, size: f32) -> Box<dyn Element> {
    const PATHS: [&str; SPINNER_FRAMES] = [
        "icons/spinner-00.svg",
        "icons/spinner-01.svg",
        "icons/spinner-02.svg",
        "icons/spinner-03.svg",
        "icons/spinner-04.svg",
        "icons/spinner-05.svg",
        "icons/spinner-06.svg",
        "icons/spinner-07.svg",
        "icons/spinner-08.svg",
        "icons/spinner-09.svg",
        "icons/spinner-10.svg",
        "icons/spinner-11.svg",
    ];
    ConstrainedBox::new(Icon::new(PATHS[frame % SPINNER_FRAMES], color).finish())
        .with_width(size)
        .with_height(size)
        .finish()
}
