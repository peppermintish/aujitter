//! One tray per signed-in user. A second launch requests the existing window.
use crate::config;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
};

pub(super) struct DesktopInstance {
    _lock: File,
    request: PathBuf,
}

impl DesktopInstance {
    pub(super) fn acquire(background: bool) -> Result<Option<Self>> {
        Self::acquire_in(config::data_dir(), background)
    }

    fn acquire_in(folder: PathBuf, background: bool) -> Result<Option<Self>> {
        std::fs::create_dir_all(&folder)?;
        let request = folder.join("desktop-open.request");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(folder.join("desktop.lock"))?;
        match lock.try_lock_exclusive() {
            Ok(()) => {
                // Ignore a request left behind when an earlier desktop process ended.
                let _ = std::fs::remove_file(&request);
                Ok(Some(Self {
                    _lock: lock,
                    request,
                }))
            }
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                if !background {
                    std::fs::write(&request, b"open")?;
                }
                Ok(None)
            }
            Err(error) => Err(error).context("Could not reserve the AuJitter desktop session"),
        }
    }

    pub(super) fn request_path(&self) -> PathBuf {
        self.request.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_launch_requests_open_without_creating_another_tray() {
        let folder = tempfile::tempdir().unwrap();
        let first = DesktopInstance::acquire_in(folder.path().into(), true)
            .unwrap()
            .unwrap();
        assert!(
            DesktopInstance::acquire_in(folder.path().into(), false)
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read(first.request_path()).unwrap(), b"open");
        drop(first);
        assert!(
            DesktopInstance::acquire_in(folder.path().into(), true)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn duplicate_background_launch_does_not_open_a_window() {
        let folder = tempfile::tempdir().unwrap();
        let first = DesktopInstance::acquire_in(folder.path().into(), true)
            .unwrap()
            .unwrap();
        assert!(
            DesktopInstance::acquire_in(folder.path().into(), true)
                .unwrap()
                .is_none()
        );
        assert!(!first.request_path().exists());
    }
}
