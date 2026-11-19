//! Settings filesystem helpers.

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dragent_paths::home()
}
