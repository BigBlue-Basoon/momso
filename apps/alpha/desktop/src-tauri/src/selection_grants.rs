//! Short-lived host-recorded selections, never supplied by the webview itself.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri_plugin_dialog::DialogExt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Purpose {
    Source,
    Conversation,
    Archive,
}

#[derive(Clone, Default)]
pub(crate) struct Selections(Arc<Mutex<HashMap<(String, Purpose), Grant>>>);

struct Grant {
    canonical: PathBuf,
    expiry: Instant,
    id: uuid::Uuid,
    in_use: bool,
    dropped_folder: bool,
}

pub(crate) struct SelectionUse {
    selections: Selections,
    key: (String, Purpose),
    id: uuid::Uuid,
    succeeded: bool,
}
impl SelectionUse {
    pub(crate) fn succeed(mut self) {
        self.succeeded = true;
    }
}
impl Drop for SelectionUse {
    fn drop(&mut self) {
        if let Ok(mut grants) = self.selections.0.lock() {
            if let Some(grant) = grants.get_mut(&self.key) {
                if grant.id == self.id {
                    if self.succeeded {
                        grants.remove(&self.key);
                    } else {
                        grant.in_use = false;
                    }
                }
            }
        }
    }
}

fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        text.replace('/', "\\").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        text.into_owned()
    }
}

impl Selections {
    pub(crate) fn record_window_event(&self, event: &tauri::WindowEvent) {
        if let tauri::WindowEvent::DragDrop(event) = event {
            self.record_drop(event);
        }
    }

    pub(crate) fn record_drop(&self, event: &tauri::DragDropEvent) {
        if let tauri::DragDropEvent::Drop { paths, .. } = event {
            self.remember_inner(paths, Purpose::Source, true);
        }
    }
    pub(crate) fn remember(&self, paths: &[PathBuf], purpose: Purpose) {
        self.remember_inner(paths, purpose, false);
    }

    fn remember_inner(&self, paths: &[PathBuf], purpose: Purpose, record_folders: bool) {
        let Ok(mut grants) = self.0.lock() else {
            return;
        };
        let now = Instant::now();
        grants.retain(|_, grant| grant.expiry > now);
        for path in paths {
            if let Ok(canonical) = path.canonicalize() {
                let correct_type = if matches!(purpose, Purpose::Archive) {
                    canonical.is_dir()
                } else {
                    canonical.is_file()
                };
                let dropped_folder = record_folders && canonical.is_dir();
                if correct_type || dropped_folder {
                    grants.insert(
                        (path_key(path), purpose),
                        Grant {
                            canonical,
                            expiry: now + Duration::from_secs(300),
                            id: uuid::Uuid::new_v4(),
                            in_use: false,
                            dropped_folder,
                        },
                    );
                }
            }
        }
    }

    pub(crate) fn check(&self, path: &str, purpose: Purpose, consume: bool) -> Result<(), String> {
        let mut grants = self.0.lock().map_err(|_| "selection_required".to_owned())?;
        let key = (path_key(Path::new(path)), purpose);
        let Some(grant) = grants.get(&key) else {
            return Err("selection_required".into());
        };
        if grant.expiry <= Instant::now()
            || Path::new(path).canonicalize().ok().as_ref() != Some(&grant.canonical)
        {
            grants.remove(&key);
            return Err("selection_required".into());
        }
        if grant.dropped_folder {
            return Err("source_folder_not_allowed".into());
        }
        if consume {
            grants.remove(&key);
        }
        Ok(())
    }

    pub(crate) async fn check_source_wait(&self, path: &str) -> Result<(), String> {
        for attempt in 0..=10 {
            match self.check(path, Purpose::Source, false) {
                Err(error) if error == "selection_required" && attempt < 10 => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                result => return result,
            }
        }
        unreachable!()
    }

    pub(crate) fn begin(&self, path: &str, purpose: Purpose) -> Result<SelectionUse, String> {
        self.check(path, purpose, false)?;
        let key = (path_key(Path::new(path)), purpose);
        let mut grants = self.0.lock().map_err(|_| "selection_required".to_owned())?;
        let grant = grants.get_mut(&key).ok_or("selection_required")?;
        if grant.in_use || grant.expiry <= Instant::now() {
            return Err("selection_required".into());
        }
        grant.in_use = true;
        Ok(SelectionUse {
            selections: self.clone(),
            key,
            id: grant.id,
            succeeded: false,
        })
    }
}

#[tauri::command]
pub(crate) async fn select_local_paths(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    purpose: String,
) -> Result<Vec<String>, String> {
    let purpose = match purpose.as_str() {
        "source" => Purpose::Source,
        "conversation" => Purpose::Conversation,
        "archive" => Purpose::Archive,
        _ => return Err("invalid_selection_purpose".into()),
    };
    let selected = tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        let files = match purpose {
            Purpose::Source => dialog.blocking_pick_files().unwrap_or_default(),
            Purpose::Conversation => dialog
                .add_filter("Conversation", &["json", "jsonl", "zip"])
                .blocking_pick_files()
                .unwrap_or_default(),
            Purpose::Archive => dialog.blocking_pick_folder().into_iter().collect(),
        };
        files
            .into_iter()
            .filter_map(|p| p.into_path().ok())
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|_| "unavailable".to_owned())?;
    state.selections.remember(&selected, purpose);
    Ok(selected
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grants_are_scoped_and_consumed_after_preview() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("fixture.txt");
        std::fs::write(&file, "fixture").unwrap();
        let s = Selections::default();
        let value = file.to_str().unwrap();
        assert!(s.check(value, Purpose::Source, false).is_err());
        s.remember(std::slice::from_ref(&file), Purpose::Source);
        assert!(s.check(value, Purpose::Conversation, false).is_err());
        let unselected_alias = temp.path().join("missing").join("..").join("fixture.txt");
        assert!(s
            .check(unselected_alias.to_str().unwrap(), Purpose::Source, false)
            .is_err());
        assert!(s
            .check(
                "\\\\unselected.invalid\\share\\file",
                Purpose::Source,
                false
            )
            .is_err());
        assert!(s.check(value, Purpose::Source, false).is_ok());
        assert!(s.check(value, Purpose::Source, true).is_ok());
        assert!(s.check(value, Purpose::Source, false).is_err());
    }

    #[test]
    fn failed_attempt_can_retry_and_success_consumes_only_that_file() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.txt");
        let second = temp.path().join("second.txt");
        std::fs::write(&first, "one").unwrap();
        std::fs::write(&second, "two").unwrap();
        let selections = Selections::default();
        selections.remember(&[first.clone(), second.clone()], Purpose::Source);
        let path = first.to_str().unwrap();
        let attempt = selections.begin(path, Purpose::Source).unwrap();
        assert!(selections.begin(path, Purpose::Source).is_err());
        drop(attempt);
        selections.begin(path, Purpose::Source).unwrap().succeed();
        assert!(selections.begin(path, Purpose::Source).is_err());
        assert!(selections
            .begin(second.to_str().unwrap(), Purpose::Source)
            .is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn selected_file_rejects_replaced_parent_junction() {
        use std::os::windows::process::CommandExt;
        let temp = tempfile::tempdir().unwrap();
        let chosen = temp.path().join("chosen");
        let moved = temp.path().join("moved");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&chosen).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let path = chosen.join("file.txt");
        std::fs::write(&path, "selected").unwrap();
        std::fs::write(outside.join("file.txt"), "different").unwrap();
        let selections = Selections::default();
        selections.remember(std::slice::from_ref(&path), Purpose::Source);
        std::fs::rename(&chosen, &moved).unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&chosen)
            .arg(&outside)
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(selections
            .begin(path.to_str().unwrap(), Purpose::Source)
            .is_err());
    }

    #[test]
    fn folder_grant_is_exact_scoped_and_expires() {
        let temp = tempfile::tempdir().unwrap();
        let selected = temp.path().join("selected");
        let other = temp.path().join("other");
        std::fs::create_dir_all(&selected).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let s = Selections::default();
        s.remember(std::slice::from_ref(&selected), Purpose::Archive);
        assert!(s
            .check(selected.to_str().unwrap(), Purpose::Archive, false)
            .is_ok());
        assert!(s
            .check(other.to_str().unwrap(), Purpose::Archive, false)
            .is_err());
        assert!(s
            .check(selected.to_str().unwrap(), Purpose::Conversation, false)
            .is_err());
        let key = (path_key(&selected), Purpose::Archive);
        s.0.lock().unwrap().get_mut(&key).unwrap().expiry = Instant::now() - Duration::from_secs(1);
        assert!(s
            .check(selected.to_str().unwrap(), Purpose::Archive, false)
            .is_err());
    }

    #[test]
    fn native_drop_event_grants_only_dropped_files() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("drop.txt");
        std::fs::write(&file, "fixture").unwrap();
        let selections = Selections::default();
        let paths = vec![file.clone()];
        let position = tauri::PhysicalPosition::new(0.0, 0.0);
        selections.record_drop(&tauri::DragDropEvent::Enter {
            paths: paths.clone(),
            position,
        });
        assert!(selections
            .check(file.to_str().unwrap(), Purpose::Source, false)
            .is_err());
        selections.record_drop(&tauri::DragDropEvent::Drop { paths, position });
        assert!(selections
            .check(file.to_str().unwrap(), Purpose::Source, true)
            .is_ok());
        assert!(selections
            .check(file.to_str().unwrap(), Purpose::Source, false)
            .is_err());
    }

    #[test]
    fn window_drop_event_grants_source_selection() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("drop.txt");
        std::fs::write(&file, "fixture").unwrap();
        let selections = Selections::default();
        let event = tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop {
            paths: vec![file.clone()],
            position: tauri::PhysicalPosition::new(0.0, 0.0),
        });
        selections.record_window_event(&event);
        assert!(selections
            .check(file.to_str().unwrap(), Purpose::Source, true)
            .is_ok());
    }

    #[tokio::test]
    async fn delayed_drop_is_accepted_and_missing_selection_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("drop.txt");
        std::fs::write(&file, "fixture").unwrap();
        let selections = Selections::default();
        let later = selections.clone();
        let dropped = file.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            later.record_drop(&tauri::DragDropEvent::Drop {
                paths: vec![dropped],
                position: tauri::PhysicalPosition::new(0.0, 0.0),
            });
        });
        assert!(selections
            .check_source_wait(file.to_str().unwrap())
            .await
            .is_ok());
        task.await.unwrap();
        let missing = temp.path().join("missing.txt");
        assert_eq!(
            selections
                .check_source_wait(missing.to_str().unwrap())
                .await,
            Err("selection_required".into())
        );
    }

    #[tokio::test]
    async fn mixed_drop_allows_file_and_reports_folder() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        let folder = temp.path().join("folder");
        std::fs::write(&file, "fixture").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let selections = Selections::default();
        selections.record_drop(&tauri::DragDropEvent::Drop {
            paths: vec![file.clone(), folder.clone()],
            position: tauri::PhysicalPosition::new(0.0, 0.0),
        });
        assert_eq!(
            selections.check_source_wait(folder.to_str().unwrap()).await,
            Err("source_folder_not_allowed".into())
        );
        assert!(selections
            .check_source_wait(file.to_str().unwrap())
            .await
            .is_ok());
        assert!(selections
            .begin(file.to_str().unwrap(), Purpose::Source)
            .is_ok());
    }
}
