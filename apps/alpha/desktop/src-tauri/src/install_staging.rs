//! App-owned installation scratch space. Never reuses or removes unknown paths.
#[cfg(windows)]
use std::io::Read;
use std::{fs, io::Write, path::Path};

const MARKER: &str = ".momso-stage-owner";
const OWNER: &str = "momso-install-stage-v1";

pub(crate) struct OwnedStage {
    // Close the lock before TempDir removes its own tree on Windows.
    lock: Option<fs::File>,
    dir: Option<tempfile::TempDir>,
}

impl OwnedStage {
    pub(crate) fn new(parent: &Path, prefix: &str) -> std::io::Result<Self> {
        fs::create_dir_all(parent)?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
            return Err(std::io::Error::other("invalid staging directory"));
        }
        cleanup(parent, prefix);
        let dir = tempfile::Builder::new().prefix(prefix).tempdir_in(parent)?;
        let mut opts = fs::OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0);
        }
        let mut lock = opts.open(dir.path().join(MARKER))?;
        lock.write_all(OWNER.as_bytes())?;
        lock.sync_all()?;
        Ok(Self {
            lock: Some(lock),
            dir: Some(dir),
        })
    }
    pub(crate) fn path(&self) -> &Path {
        self.dir.as_ref().expect("owned stage exists").path()
    }
    pub(crate) fn mark_process_started(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom};
        let file = self
            .lock
            .as_mut()
            .ok_or_else(|| std::io::Error::other("stage unavailable"))?;
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"momso-install-process-may-be-active")?;
        file.sync_all()
    }
    pub(crate) fn retain(&mut self) {
        self.lock.take();
        if let Some(dir) = self.dir.take() {
            let _ = dir.keep();
        }
    }
}
impl Drop for OwnedStage {
    fn drop(&mut self) {
        self.lock.take();
        self.dir.take();
    }
}

#[cfg(windows)]
fn cleanup(parent: &Path, prefix: &str) {
    use std::os::windows::fs::OpenOptionsExt;
    let Ok(parent_absolute) = fs::canonicalize(parent) else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(prefix) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
            continue;
        }
        let Ok(resolved) = fs::canonicalize(&path) else {
            continue;
        };
        if resolved.parent() != Some(parent_absolute.as_path()) {
            continue;
        }
        let safe = walkdir::WalkDir::new(&path)
            .follow_links(false)
            .into_iter()
            .all(|entry| {
                entry
                    .ok()
                    .and_then(|entry| fs::symlink_metadata(entry.path()).ok())
                    .is_some_and(|metadata| !crate::archive_location::is_link_or_reparse(&metadata))
            });
        if !safe {
            continue;
        }
        let marker_path = path.join(MARKER);
        let Ok(meta) = fs::symlink_metadata(&marker_path) else {
            continue;
        };
        if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
            continue;
        }
        let Ok(mut lock) = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(marker_path)
        else {
            continue;
        };
        let mut content = String::new();
        if lock.read_to_string(&mut content).is_err() || content != OWNER {
            continue;
        }
        // No active installer can hold the marker; app code never reuses a stage.
        drop(lock);
        let _ = fs::remove_dir_all(&resolved);
    }
}

#[cfg(not(windows))]
fn cleanup(_parent: &Path, _prefix: &str) {}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn abandoned_owned_stage_is_removed_but_live_and_unknown_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let live = OwnedStage::new(root.path(), ".provider-install-").unwrap();
        let live_path = live.path().to_owned();
        let abandoned = root.path().join(".provider-install-abandoned");
        fs::create_dir(&abandoned).unwrap();
        fs::write(abandoned.join(MARKER), OWNER).unwrap();
        let unknown = root.path().join(".provider-install-user");
        fs::create_dir(&unknown).unwrap();
        fs::write(unknown.join("document"), b"keep").unwrap();
        let uncertain = root.path().join(".provider-install-possible-child");
        fs::create_dir(&uncertain).unwrap();
        fs::write(
            uncertain.join(MARKER),
            b"momso-install-process-may-be-active",
        )
        .unwrap();
        cleanup(root.path(), ".provider-install-");
        assert!(!abandoned.exists());
        assert!(live_path.exists());
        assert_eq!(fs::read(unknown.join("document")).unwrap(), b"keep");
        assert!(uncertain.exists());
        drop(live);
        assert!(!live_path.exists());
    }
}
