//! UI-independent agent domain: blocks, conversation state, session facts,
//! permissions, API contracts, project context, and the runtime seam
//! (`EventSink`, host traits) for the persistent Host.
//!
//! No WarpUI dependency. Presentation and live integrations (GraphQL impl,
//! terminal models, git status) live in the app crate, which re-exports
//! these paths.

pub mod api;
pub mod api_types;
pub mod approval_policy;
pub mod attachments;
pub mod autonomy;
pub mod background;
pub mod block;
pub mod checkpoint_store;
pub mod controller;
pub mod diff;
pub mod direct;
pub mod edit_diff;
pub mod error_display;
pub mod files;
pub mod grant;
pub mod permission_store;
pub mod permissions;
pub mod pipeline;
pub mod project;
pub mod runtime;
pub mod session;
pub mod session_facts;
pub mod session_store;
pub mod state;
pub mod telemetry;
pub mod types;
