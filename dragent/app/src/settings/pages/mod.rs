//! Settings page content renderers.

mod about;
pub(crate) mod account;
mod agent;
mod appearance;
mod code_index;
mod computer_use;
mod dev_permissions;
pub(crate) mod integrations;
mod keyboard;
mod mobile;
mod models;
pub(crate) mod notifications;
mod org;
mod privacy;
mod skills;
mod stub;
mod system_new;
mod terminal;
mod voice;
mod workflow;

pub use about::render_about;
pub use account::render_account;
pub use agent::{render_agent_general, render_agent_mcp, render_default_agent_menu};
pub use appearance::{render_appearance, render_theme_chooser};
pub use code_index::render_code_index;
pub use computer_use::render_computer_use;
pub use dev_permissions::render_developer_permissions;
pub use keyboard::render_keyboard_shortcuts;
pub use mobile::render_mobile;
pub use mobile::render_mobile_overlay;
pub use models::render_agent_models;
pub use integrations::{render_integration_dialog, render_integrations};
pub use org::{render_billing, render_members, render_teams};
pub use super::projects::render_projects;
pub use privacy::render_privacy;
pub use skills::render_skills;
pub(crate) use skills::{render_skill_menu, SKILL_MENU_TAG};
pub use notifications::render_notifications;
pub use system_new::render_remote_access;
pub use terminal::render_terminal_input;
pub use voice::render_voice;
pub use workflow::{
    render_agent_commands, render_editor, render_general, render_version_control,
};
