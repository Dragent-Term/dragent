//! Dragent plugins: a package that adds skills (instructions for agents) and
//! MCP servers (tools) together, with a lifecycle the person can follow.
//!
//! A package is a folder holding `dragent-plugin.json`. Inspecting one reads
//! it and nothing else: no program in it runs. Installing copies it into
//! Dragent's own folder, places its skills in the shared skills folder with
//! every file's hash on record (so updates and removal never touch a file
//! the person edited, or a skill the plugin doesn't own), and plans its
//! servers as MCP entries that start switched off: only the person turns a
//! server on, in Settings, and only the person approves its tool calls.

pub mod manifest;
pub mod package;
pub mod record;
pub mod servers;
pub mod skills;

pub use manifest::{EnvSpec, Manifest, ServerSpec, Transport, MANIFEST_FILE};
pub use record::{InstalledPlugin, Record, Source};
