use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use anyhow::{Context, Result};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::event::AppEvent;

pub struct PdfWatcher {
    _watcher: RecommendedWatcher,
}

impl PdfWatcher {
    pub fn start(path: &Path, tx: Sender<AppEvent>) -> Result<Self> {
        let target = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let target_name = target
            .file_name()
            .context("PDF path has no file name")?
            .to_owned();
        let parent = target
            .parent()
            .context("PDF path has no parent directory")?
            .to_path_buf();
        let mut watcher = notify::recommended_watcher(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) if event_targets(&event.paths, &target, &target_name) => {
                    let _ = tx.send(AppEvent::FileChanged);
                }
                Ok(_) => {}
                Err(error) => {
                    let _ = tx.send(AppEvent::WatchError(error.to_string()));
                }
            },
        )
        .context("could not create filesystem watcher")?;
        watcher
            .watch(&parent, RecursiveMode::NonRecursive)
            .with_context(|| format!("could not watch {}", parent.display()))?;
        Ok(Self { _watcher: watcher })
    }
}

fn event_targets(paths: &[PathBuf], target: &Path, target_name: &std::ffi::OsStr) -> bool {
    paths.iter().any(|path| {
        path == target
            || path
                .file_name()
                .is_some_and(|file_name| file_name == target_name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replace_paths_match_by_target_name() {
        let target = Path::new("/tmp/out/document.pdf");
        assert!(event_targets(
            &[PathBuf::from("/tmp/out/document.pdf")],
            target,
            target.file_name().unwrap()
        ));
        assert!(!event_targets(
            &[PathBuf::from("/tmp/out/.document.pdf.tmp")],
            target,
            target.file_name().unwrap()
        ));
    }
}
