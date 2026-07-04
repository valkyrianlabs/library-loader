mod config;
mod consts;
mod cse;
mod epw;
mod error;
mod format;
mod logger;
mod updates;
mod utils;
mod watcher;

use std::{path::PathBuf, sync::Arc};

pub use {
    config::{profile::Profile, Config, Format},
    consts::LL_CONFIG,
    error::{Error, Result},
    format::ECAD,
    logger::{ConsoleLogger, Logger},
    updates::check as check_updates,
    updates::{ClientKind, UpdateInfo},
    watcher::Watcher,
};

pub fn download_once<P: Into<PathBuf>>(config: &Config, path: P) -> Result<Vec<PathBuf>> {
    let formats = Arc::new(config.formats()?);
    let epw = epw::Epw::from_file(path.into())?;
    let mut saved_paths = Vec::new();

    for res in cse::CSE::new(config.profile.token(), Arc::clone(&formats)).get(epw)? {
        saved_paths.push(res.save()?);
    }

    Ok(saved_paths)
}
