use crate::archive_zet::{parse, render, valid_zet_id, validate_folder, WRITE_LOCK};

const FOLDERS_FILE: &str = ".folders.json";

pub(crate) fn folder_directory(root: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let directory = root.join("zettels");
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| "unavailable".to_string())?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unavailable".into());
    }
    Ok(directory)
}

pub(crate) fn ensure_regular_file(path: &std::path::Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "unavailable".to_string())?;
    if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unavailable".into());
    }
    Ok(())
}

pub fn read_folder_registry(
    root: &std::path::Path,
) -> Result<std::collections::BTreeSet<String>, String> {
    let path = folder_directory(root)?.join(FOLDERS_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata)
            if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) =>
        {
            return Err("invalid_folders".into())
        }
        Ok(_) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(_) => return Err("unavailable".into()),
    }
    match std::fs::read(&path) {
        Ok(data) => {
            let folders: std::collections::BTreeSet<String> =
                serde_json::from_slice(&data).map_err(|_| "invalid_folders".to_string())?;
            for folder in &folders {
                validate_folder(folder)?;
                if folder.is_empty() {
                    return Err("invalid_folders".into());
                }
            }
            Ok(folders)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(_) => Err("unavailable".into()),
    }
}

pub(crate) fn add_ancestors(folders: &mut std::collections::BTreeSet<String>, folder: &str) {
    let mut current = String::new();
    for part in folder.split('/').filter(|part| !part.is_empty()) {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);
        folders.insert(current.clone());
    }
}

pub(crate) fn write_folder_registry(
    root: &std::path::Path,
    folders: &std::collections::BTreeSet<String>,
) -> Result<(), String> {
    use std::io::Write;
    let dir = folder_directory(root)?;
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|_| "unavailable".to_string())?;
    serde_json::to_writer_pretty(&mut temp, folders).map_err(|_| "unavailable".to_string())?;
    temp.write_all(b"\n")
        .map_err(|_| "unavailable".to_string())?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "unavailable".to_string())?;
    temp.persist(dir.join(FOLDERS_FILE))
        .map_err(|_| "unavailable".to_string())?;
    Ok(())
}

pub(crate) fn document_files(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in
        std::fs::read_dir(folder_directory(root)?).map_err(|_| "unavailable".to_string())?
    {
        let entry = entry.map_err(|_| "unavailable".to_string())?;
        if entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".md"))
            .is_some_and(valid_zet_id)
        {
            ensure_regular_file(&entry.path())?;
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

pub fn folders(root: &std::path::Path) -> Result<Vec<String>, String> {
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    let mut result = read_folder_registry(root)?;
    for path in document_files(root)? {
        let text = std::fs::read_to_string(path).map_err(|_| "unavailable".to_string())?;
        let (head, _) = parse(&text)?;
        add_ancestors(&mut result, &head.folder);
    }
    Ok(result.into_iter().collect())
}

#[cfg(feature = "standalone")]
pub fn create_folder(root: &std::path::Path, folder: &str) -> Result<(), String> {
    validate_folder(folder)?;
    if folder.is_empty() {
        return Err("invalid_folder".into());
    }
    let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_owned())?;
    let mut registry = read_folder_registry(root)?;
    add_ancestors(&mut registry, folder);
    write_folder_registry(root, &registry)
}

fn document_with_folder(text: &str, folder: &str) -> Result<String, String> {
    let (mut head, body) = parse(text)?;
    head.folder = folder.to_owned();
    render(&head, body)
}

fn replace_document(path: &std::path::Path, expected: &str, content: &str) -> Result<(), String> {
    use std::io::Write;
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().ok_or("unavailable")?)
        .map_err(|_| "unavailable".to_string())?;
    temp.write_all(content.as_bytes())
        .map_err(|_| "unavailable".to_string())?;
    temp.as_file()
        .sync_all()
        .map_err(|_| "unavailable".to_string())?;
    if std::fs::read_to_string(path).map_err(|_| "unavailable".to_string())? != expected {
        return Err("conflict".into());
    }
    temp.persist(path).map_err(|_| "unavailable".to_string())?;
    Ok(())
}

#[cfg(not(feature = "standalone"))]
pub(crate) fn write_document_folder(
    path: &std::path::Path,
    text: &str,
    folder: &str,
) -> Result<(), String> {
    replace_document(path, text, &document_with_folder(text, folder)?)
}

/// Caller holds WRITE_LOCK and has checked its own write policy.
pub(crate) fn rename_folder_locked(
    root: &std::path::Path,
    from: &str,
    to: &str,
) -> Result<(), String> {
    validate_folder(from)?;
    validate_folder(to)?;
    if from.is_empty() || to.is_empty() || to.starts_with(&format!("{from}/")) {
        return Err("invalid_folder".into());
    }
    let mut registry = read_folder_registry(root)?;
    let mut changes = Vec::new();
    for path in document_files(root)? {
        let original = std::fs::read_to_string(&path).map_err(|_| "unavailable".to_string())?;
        let (head, _) = parse(&original)?;
        if path.file_name().and_then(|name| name.to_str())
            != Some(format!("{}.md", head.id).as_str())
        {
            return Err("invalid_document".into());
        }
        let suffix = head
            .folder
            .strip_prefix(from)
            .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'));
        if let Some(suffix) = suffix {
            let updated = document_with_folder(&original, &format!("{to}{suffix}"))?;
            changes.push((path, original, updated));
        }
    }
    let matching: Vec<_> = registry
        .iter()
        .filter(|f| *f == from || f.starts_with(&format!("{from}/")))
        .cloned()
        .collect();
    if matching.is_empty() && changes.is_empty() {
        return Err("not_found".into());
    }
    for folder in matching {
        let renamed = format!("{to}{}", &folder[from.len()..]);
        validate_folder(&renamed)?;
        registry.remove(&folder);
        add_ancestors(&mut registry, &renamed);
    }
    for (_, _, updated) in &changes {
        add_ancestors(&mut registry, &parse(updated)?.0.folder);
    }
    let mut applied: Vec<(std::path::PathBuf, String, String)> = Vec::new();
    for (path, original, updated) in changes {
        if let Err(error) = replace_document(&path, &original, &updated) {
            for (path, original, updated) in applied.iter().rev() {
                let _ = replace_document(path, updated, original);
            }
            return Err(error);
        }
        applied.push((path, original, updated));
    }
    if let Err(error) = write_folder_registry(root, &registry) {
        for (path, original, updated) in applied.iter().rev() {
            let _ = replace_document(path, updated, original);
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(feature = "standalone")]
pub fn rename_folder(root: &std::path::Path, from: &str, to: &str) -> Result<(), String> {
    let _file_guard = crate::lock::exclusive(root).map_err(str::to_owned)?;
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    rename_folder_locked(root, from, to)
}
