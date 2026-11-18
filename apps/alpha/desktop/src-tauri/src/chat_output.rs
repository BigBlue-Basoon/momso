use std::path::{Path, PathBuf};

pub(crate) fn create(root: &Path, chat_id: &str) -> Result<PathBuf, String> {
    if chat_id.is_empty()
        || !chat_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err("invalid_chat_id".into());
    }
    if !root.is_absolute() {
        return Err("invalid_output_directory".into());
    }
    let canonical_root = root.canonicalize().map_err(|_| "unavailable")?;
    let mut path = root.to_path_buf();
    let turn = uuid::Uuid::new_v4().to_string();
    for component in [".momso", "outputs", chat_id, turn.as_str()] {
        path.push(component);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && component != turn => {}
            Err(_) => return Err("unavailable".into()),
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| "unavailable")?;
        if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
            return Err("invalid_output_directory".into());
        }
        if !path
            .canonicalize()
            .map_err(|_| "unavailable")?
            .starts_with(&canonical_root)
        {
            return Err("invalid_output_directory".into());
        }
    }
    Ok(path)
}

pub(crate) fn instruction(path: &Path) -> Result<String, String> {
    let path = path.to_str().ok_or("unavailable")?;
    let quoted = serde_json::to_string(path).map_err(|_| "unavailable")?;
    Ok(format!("\n\nSave generated images, videos, and exported files in this turn's output directory: {quoted}. After creating an image or video, call media_register_generated with its absolute path and the actual generation prompt; supply regenerateOf when applicable."))
}

pub(crate) fn add_dir(command: &mut tokio::process::Command, path: &Path) {
    command.arg("--add-dir").arg(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_turn_output_directory_and_cli_arguments() {
        let root = tempfile::tempdir().unwrap();
        let first = create(root.path(), "chat_abc-123").unwrap();
        let second = create(root.path(), "chat_abc-123").unwrap();
        assert_ne!(first, second);
        assert!(first.is_dir());
        assert_eq!(first.parent().unwrap().file_name().unwrap(), "chat_abc-123");
        let instruction = instruction(&first).unwrap();
        assert!(instruction.contains("output directory"));
        assert!(instruction.contains(&serde_json::to_string(first.to_str().unwrap()).unwrap()));
        assert!(create(root.path(), "../outside").is_err());
        for cli in ["claude", "agy"] {
            let mut command = tokio::process::Command::new(cli);
            add_dir(&mut command, &first);
            let args: Vec<_> = command.as_std().get_args().collect();
            assert_eq!(
                args,
                vec![std::ffi::OsStr::new("--add-dir"), first.as_os_str()]
            );
        }
        let sandbox = crate::chat_codex::turn_sandbox("workspaceWrite", root.path(), Some(&first));
        assert_eq!(sandbox["writableRoots"][1], first.to_str().unwrap());
        let full_access =
            crate::chat_codex::turn_sandbox("dangerFullAccess", root.path(), Some(&first));
        assert_eq!(full_access["type"], "dangerFullAccess");
        assert!(full_access.get("writableRoots").is_none());
    }
}
