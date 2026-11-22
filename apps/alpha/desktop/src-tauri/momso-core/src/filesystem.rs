//! Headless local archive primitives. The caller serializes archive writes.
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

fn ensure_directory(path: &Path) -> Result<(), &'static str> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = fs::symlink_metadata(path).map_err(|_| "io_error")?;
            if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
                return Err("unsafe_path");
            }
            Ok(())
        }
        Err(_) => Err("io_error"),
    }
}

/// Initialize an empty root without overwriting existing user files.
pub fn initialize(root: &Path) -> Result<(), String> {
    ensure_directory(root).map_err(str::to_owned)?;
    for name in ["zettels", "objects", "receipts", "skills"] {
        ensure_directory(&root.join(name)).map_err(str::to_owned)?;
    }
    crate::git::initialize(root).map_err(str::to_owned)?;
    crate::dictionary::initialize(root)?;
    install_bundled_skills(root).map_err(str::to_owned)
}

/// Uses desktop's transactional installer, preserving user edits and state.
pub fn install_bundled_skills(root: &Path) -> Result<(), &'static str> {
    crate::skill_storage::install(root)
}

pub fn read_skill(
    root: &Path,
    name: &str,
) -> Result<(crate::skill::SkillHead, String), &'static str> {
    if !crate::skill::valid_name(name) {
        return Err("invalid_name");
    }
    match crate::skill_storage::reconcile(root, name) {
        Ok(()) | Err("conflict") => (),
        Err(error) => return Err(error),
    }
    inspect_skill(root, name)
}

/// Read a skill without reconciling provider copies or mutating files.
pub fn inspect_skill(
    root: &Path,
    name: &str,
) -> Result<(crate::skill::SkillHead, String), &'static str> {
    if !crate::skill::valid_name(name) {
        return Err("invalid_name");
    }
    let skills = root.join("skills");
    let directory = skills.join(name);
    for path in [&skills, &directory] {
        let meta = fs::symlink_metadata(path).map_err(|_| "not_found")?;
        if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
            return Err("unsafe_path");
        }
    }
    let path = directory.join("SKILL.md");
    let meta = fs::symlink_metadata(&path).map_err(|_| "not_found")?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    let text = fs::read_to_string(path).map_err(|_| "io_error")?;
    crate::skill::parse(name, &text)
}

/// AI invocation requires both enabled provider copies to match the canonical skill.
pub fn read_callable_skill(
    root: &Path,
    name: &str,
) -> Result<(crate::skill::SkillHead, String), &'static str> {
    let parsed = read_skill(root, name)?;
    inspect_skill_copies(root, name, parsed)
}

/// Check whether a skill is callable without repairing provider copies.
pub fn inspect_callable_skill(
    root: &Path,
    name: &str,
) -> Result<(crate::skill::SkillHead, String), &'static str> {
    let parsed = inspect_skill(root, name)?;
    inspect_skill_copies(root, name, parsed)
}

fn inspect_skill_copies(
    root: &Path,
    name: &str,
    parsed: (crate::skill::SkillHead, String),
) -> Result<(crate::skill::SkillHead, String), &'static str> {
    let canonical = crate::skill::skill_path(root, "skills", name)?;
    let content = fs::read_to_string(canonical).map_err(|_| "io_error")?;
    let mut found = 0;
    for folder in [".agents/skills", ".claude/skills"] {
        let path = crate::skill::skill_path(root, folder, name)?;
        match fs::read_to_string(path) {
            Ok(copy) if copy == content => found += 1,
            Ok(_) => return Err("skill_conflict"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err("io_error"),
        }
    }
    match found {
        2 => Ok(parsed),
        0 => Err("disabled"),
        _ => Err("skill_conflict"),
    }
}

fn document_path(root: &Path, id: &str) -> Result<PathBuf, &'static str> {
    if !crate::document::valid_zet_id(id) {
        return Err("invalid_id");
    }
    let directory = root.join("zettels");
    let meta = fs::symlink_metadata(&directory).map_err(|_| "not_found")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    Ok(directory.join(format!("{id}.md")))
}

pub fn read_document(root: &Path, id: &str) -> Result<String, &'static str> {
    let path = document_path(root, id)?;
    let meta = fs::symlink_metadata(&path).map_err(|_| "not_found")?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("unsafe_path");
    }
    let text = fs::read_to_string(path).map_err(|_| "io_error")?;
    let (head, _) = crate::document::parse(&text).map_err(|_| "invalid_document")?;
    if head.id != id {
        return Err("invalid_document");
    }
    Ok(text)
}

/// Creates a new document only. Indexing and Git history remain host duties.
pub fn create_document(
    root: &Path,
    head: &crate::document::ZetHead,
    body: &str,
) -> Result<(), &'static str> {
    let _file_guard = crate::lock::exclusive(root)?;
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    let path = document_path(root, &head.id)?;
    let content = crate::document::render(head, body).map_err(|_| "invalid_document")?;
    if !head.folder.is_empty() {
        crate::document::validate_folder(&head.folder).map_err(|_| "invalid_document")?;
        let mut registry = crate::folders::read_folder_registry(root).map_err(|_| "io_error")?;
        crate::folders::add_ancestors(&mut registry, &head.folder);
        crate::folders::write_folder_registry(root, &registry).map_err(|_| "io_error")?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "already_exists"
            } else {
                "io_error"
            }
        })?;
    if file
        .write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .is_err()
    {
        let _ = fs::remove_file(path);
        return Err("io_error");
    }
    Ok(())
}

/// Replace only the version the caller inspected. The server owns the root.
pub fn update_document(
    root: &Path,
    head: &crate::document::ZetHead,
    body: &str,
    expected_hash: &str,
) -> Result<(), &'static str> {
    let _file_guard = crate::lock::exclusive(root)?;
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "io_error")?;
    let current = read_document(root, &head.id)?;
    if crate::document::content_hash(&current) != expected_hash {
        return Err("conflict");
    }
    let path = document_path(root, &head.id)?;
    let content = crate::document::render(head, body).map_err(|_| "invalid_document")?;
    if !head.folder.is_empty() {
        crate::document::validate_folder(&head.folder).map_err(|_| "invalid_document")?;
        let mut registry = crate::folders::read_folder_registry(root).map_err(|_| "io_error")?;
        crate::folders::add_ancestors(&mut registry, &head.folder);
        crate::folders::write_folder_registry(root, &registry).map_err(|_| "io_error")?;
    }
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().ok_or("io_error")?)
        .map_err(|_| "io_error")?;
    temp.write_all(content.as_bytes())
        .and_then(|()| temp.as_file().sync_all())
        .map_err(|_| "io_error")?;
    temp.persist(path).map_err(|_| "io_error")?;
    Ok(())
}
