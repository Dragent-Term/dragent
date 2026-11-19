//! Shape workspace views (hub).
//!
//! Presentation helpers for the Shape surface, split by area:
//! navigation/activity, plan surfaces, record editing, composer, roles.
//! All  paths keep working through re-exports.

pub(crate) use super::shape_view_composer::*;
pub(crate) use super::shape_view_edit::*;
pub(crate) use super::shape_view_nav::*;
pub(crate) use super::shape_view_plan::*;
pub(crate) use super::shape_view_roles::*;
