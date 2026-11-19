//! Turn raw agent/provider errors into calm, user-facing messages.
//!
//! Technical detail (GraphQL locations, provider URLs, internal wrapping) stays
//! in the logs; the user sees something they can act on — especially for the
//! common, expected cases like hitting a usage limit.

pub use dragent_agent::error_display::humanize_agent_error;
