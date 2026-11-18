use serde::{Deserialize, Serialize};

pub(crate) const EDGE_TYPES: [&str; 5] = [
    "summarizes",
    "derived_from",
    "continues",
    "references",
    "supersedes",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Edge {
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) target: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ZetHead {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) assets: Vec<String>,
    pub(crate) edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) folder: String,
    #[serde(skip)]
    original_header: Option<String>,
}

pub(crate) fn valid_zet_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    bytes.len() == 19
        && &bytes[..4] == b"zet_"
        && bytes[12] == b'_'
        && bytes[4..12]
            .iter()
            .chain(bytes[13..19].iter())
            .all(u8::is_ascii_digit)
        && chrono::NaiveDateTime::parse_from_str(&id[4..], "%Y%m%d_%H%M%S").is_ok()
}

pub(crate) fn valid_asset_id(asset: &str) -> bool {
    asset.len() == 71
        && asset.starts_with("sha256:")
        && asset[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub(crate) fn validate(head: &ZetHead) -> Result<(), String> {
    validate_folder(&head.folder)?;
    if !valid_zet_id(&head.id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    if head.title.trim().is_empty() {
        return Err("제목을 입력해 주세요".into());
    }
    if head.summary.trim().is_empty() {
        return Err("요약을 입력해 주세요".into());
    }
    if !["summary", "transcript", "note"].contains(&head.kind.as_str()) {
        return Err("글 종류가 올바르지 않습니다".into());
    }
    if !["draft", "confirmed"].contains(&head.status.as_str()) {
        return Err("글 상태가 올바르지 않습니다".into());
    }
    if chrono::DateTime::parse_from_rfc3339(&head.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&head.updated_at).is_err()
    {
        return Err("글의 시간이 올바르지 않습니다".into());
    }
    for asset in &head.assets {
        if !valid_asset_id(asset) {
            return Err("원본 번호가 올바르지 않습니다".into());
        }
    }
    for edge in &head.edges {
        if !EDGE_TYPES.contains(&edge.kind.as_str()) || !valid_zet_id(&edge.target) {
            return Err("글의 연결이 올바르지 않습니다".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_folder(folder: &str) -> Result<(), String> {
    if folder.is_empty() {
        return Ok(());
    }
    if folder.len() > 4096
        || folder.chars().any(char::is_control)
        || folder
            .split('/')
            .any(|part| part.is_empty() || part.trim() != part || part == ".." || part == ".")
        || folder.contains('\\')
    {
        return Err("invalid_folder".into());
    }
    Ok(())
}

pub(crate) fn parse(text: &str) -> Result<(ZetHead, &str), String> {
    let (remaining, boundary) = if let Some(text) = text.strip_prefix("---\r\n") {
        (text, "\r\n---\r\n")
    } else if let Some(text) = text.strip_prefix("---\n") {
        (text, "\n---\n")
    } else {
        return Err("글 머리가 올바르지 않습니다".into());
    };
    let (yaml, body) = remaining
        .split_once(boundary)
        .ok_or_else(|| "글 머리가 올바르지 않습니다".to_string())?;
    let fields: serde_yaml::Value =
        serde_yaml::from_str(yaml).map_err(|_| "글 머리가 올바르지 않습니다".to_string())?;
    let keys: Vec<&str> = fields
        .as_mapping()
        .ok_or_else(|| "글 머리가 올바르지 않습니다".to_string())?
        .keys()
        .map(|key| key.as_str().unwrap_or(""))
        .collect();
    let expected = [
        "id",
        "title",
        "summary",
        "kind",
        "status",
        "created_at",
        "updated_at",
        "assets",
        "edges",
    ];
    let without_folder: Vec<_> = keys
        .iter()
        .copied()
        .filter(|key| *key != "folder")
        .collect();
    if without_folder != expected || keys.len() > expected.len() + 1 {
        return Err("글 머리 항목이나 순서가 올바르지 않습니다".into());
    }
    let mut head: ZetHead =
        serde_yaml::from_str(yaml).map_err(|_| "글 머리가 올바르지 않습니다".to_string())?;
    validate(&head)?;
    head.original_header = Some(text[..text.len() - body.len()].to_string());
    Ok((head, body))
}

pub(crate) fn render(head: &ZetHead, body: &str) -> Result<String, String> {
    validate(head)?;
    if let Some(original) = &head.original_header {
        let source = format!("{original}{body}");
        if let Ok((mut previous, _)) = parse(&source) {
            let mut current = head.clone();
            previous.original_header = None;
            current.original_header = None;
            if previous == current {
                return Ok(source);
            }
        }
    }
    let yaml = serde_yaml::to_string(head).map_err(|_| "글을 만들지 못했습니다".to_string())?;
    Ok(format!("---\n{}---\n{}", yaml, body))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ZetCreate {
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) kind: String,
    pub(crate) body: String,
    pub(crate) assets: Vec<String>,
    pub(crate) edges: Vec<Edge>,
}

#[cfg(test)]
pub(crate) fn create(
    root: &std::path::Path,
    database: &std::path::Path,
    input: ZetCreate,
) -> Result<String, String> {
    create_in_folder(root, database, input, "")
}

pub(crate) fn create_in_folder(
    root: &std::path::Path,
    database: &std::path::Path,
    input: ZetCreate,
    folder: &str,
) -> Result<String, String> {
    use std::io::Write;
    let sample = ZetHead {
        id: "zet_20000101_000000".into(),
        title: input.title.clone(),
        summary: input.summary.clone(),
        kind: input.kind.clone(),
        status: "draft".into(),
        created_at: "2000-01-01T00:00:00+00:00".into(),
        updated_at: "2000-01-01T00:00:00+00:00".into(),
        assets: input.assets.clone(),
        edges: input.edges.clone(),
        folder: folder.to_owned(),
        original_header: None,
    };
    validate(&sample)?;
    let _guard = WRITE_LOCK
        .lock()
        .map_err(|_| "글을 저장하지 못했습니다".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    if !folder.is_empty() {
        let mut registry = read_folder_registry(root)?;
        add_ancestors(&mut registry, folder);
        write_folder_registry(root, &registry)?;
    }
    let (id, stamp, mut file) = crate::archive::allocate_file(&root.join("zettels"), "zet")?;
    let head = ZetHead {
        id: id.clone(),
        created_at: stamp.clone(),
        updated_at: stamp,
        ..sample
    };
    let content = render(&head, &input.body)?;
    file.write_all(content.as_bytes())
        .map_err(|_| "글을 저장하지 못했습니다".to_string())?;
    file.sync_all()
        .map_err(|_| "글을 저장하지 못했습니다".to_string())?;
    let relative = format!("zettels/{id}.md");
    let mut conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "글 색인을 열지 못했습니다".to_string())?;
    let tx = conn
        .transaction()
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    tx.execute("INSERT INTO archive_zets(id,title,summary,kind,status,created_at,updated_at,file_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", rusqlite::params![head.id,head.title,head.summary,head.kind,head.status,head.created_at,head.updated_at,relative]).map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    for asset in &head.assets {
        tx.execute(
            "INSERT OR IGNORE INTO archive_zets_assets(zet_id,asset_id) VALUES (?1,?2)",
            rusqlite::params![id, asset],
        )
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    }
    for edge in &head.edges {
        tx.execute(
            "INSERT OR IGNORE INTO archive_zets_edges(zet_id,edge_type,target_id) VALUES (?1,?2,?3)",
            rusqlite::params![id, edge.kind, edge.target],
        )
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    }
    tx.commit()
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    Ok(id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ZetListEntry {
    id: String,
    title: String,
    summary: String,
    kind: String,
    status: String,
    created_at: String,
    updated_at: String,
    folder: String,
}

pub(crate) fn list(
    database: &std::path::Path,
    kind: Option<String>,
    status: Option<String>,
) -> Result<Vec<ZetListEntry>, String> {
    let conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "글 목록을 읽지 못했습니다".to_string())?;
    let mut statement = conn.prepare("SELECT id,title,summary,kind,status,created_at,updated_at FROM archive_zets WHERE (?1 IS NULL OR kind=?1) AND (?2 IS NULL OR status=?2) ORDER BY updated_at DESC, id DESC").map_err(|_| "글 목록을 읽지 못했습니다".to_string())?;
    let rows = statement
        .query_map(rusqlite::params![kind, status], |row| {
            Ok(ZetListEntry {
                id: row.get(0)?,
                title: row.get(1)?,
                summary: row.get(2)?,
                kind: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
                folder: String::new(),
            })
        })
        .map_err(|_| "글 목록을 읽지 못했습니다".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "글 목록을 읽지 못했습니다".into())
}

pub(crate) fn add_folders(
    root: &std::path::Path,
    entries: &mut [ZetListEntry],
) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    let directory = folder_directory(root)?;
    for entry in entries {
        if !valid_zet_id(&entry.id) {
            return Err("invalid_document".into());
        }
        let path = directory.join(format!("{}.md", entry.id));
        ensure_regular_file(&path)?;
        let text = std::fs::read_to_string(path).map_err(|_| "unavailable".to_owned())?;
        let (head, _) = parse(&text)?;
        if head.id != entry.id {
            return Err("unavailable".into());
        }
        entry.folder = head.folder;
    }
    Ok(())
}

const FOLDERS_FILE: &str = ".folders.json";

fn folder_directory(root: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let directory = root.join("zettels");
    let metadata = std::fs::symlink_metadata(&directory).map_err(|_| "unavailable".to_string())?;
    if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unavailable".into());
    }
    Ok(directory)
}

fn ensure_regular_file(path: &std::path::Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "unavailable".to_string())?;
    if !metadata.is_file() || crate::archive_location::is_link_or_reparse(&metadata) {
        return Err("unavailable".into());
    }
    Ok(())
}

pub(crate) fn read_folder_registry(
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

fn add_ancestors(folders: &mut std::collections::BTreeSet<String>, folder: &str) {
    let mut current = String::new();
    for part in folder.split('/').filter(|part| !part.is_empty()) {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(part);
        folders.insert(current.clone());
    }
}

fn write_folder_registry(
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

fn document_files(root: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in
        std::fs::read_dir(folder_directory(root)?).map_err(|_| "unavailable".to_string())?
    {
        let entry = entry.map_err(|_| "unavailable".to_string())?;
        if entry.path().extension().is_some_and(|ext| ext == "md") {
            ensure_regular_file(&entry.path())?;
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

pub(crate) fn folders(root: &std::path::Path) -> Result<Vec<String>, String> {
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    let mut result = read_folder_registry(root)?;
    for path in document_files(root)? {
        let text = std::fs::read_to_string(path).map_err(|_| "unavailable".to_string())?;
        let (head, _) = parse(&text)?;
        add_ancestors(&mut result, &head.folder);
    }
    Ok(result.into_iter().collect())
}

pub(crate) fn create_folder(
    root: &std::path::Path,
    database: &std::path::Path,
    folder: &str,
) -> Result<(), String> {
    validate_folder(folder)?;
    if folder.is_empty() {
        return Err("invalid_folder".into());
    }
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
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

fn write_document_folder(path: &std::path::Path, text: &str, folder: &str) -> Result<(), String> {
    replace_document(path, text, &document_with_folder(text, folder)?)
}

pub(crate) fn set_folder(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
    folder: &str,
) -> Result<(), String> {
    validate_folder(folder)?;
    if !valid_zet_id(id) {
        return Err("invalid_document".into());
    }
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    let path = folder_directory(root)?.join(format!("{id}.md"));
    ensure_regular_file(&path)?;
    let text = std::fs::read_to_string(&path).map_err(|_| "unavailable".to_string())?;
    let (head, _) = parse(&text)?;
    if head.id != id {
        return Err("invalid_document".into());
    }
    let mut registry = read_folder_registry(root)?;
    add_ancestors(&mut registry, &head.folder);
    add_ancestors(&mut registry, folder);
    if !registry.is_empty() {
        write_folder_registry(root, &registry)?;
    }
    write_document_folder(&path, &text, folder)
}

pub(crate) fn rename_folder(
    root: &std::path::Path,
    database: &std::path::Path,
    from: &str,
    to: &str,
) -> Result<(), String> {
    validate_folder(from)?;
    validate_folder(to)?;
    if from.is_empty() || to.is_empty() || to.starts_with(&format!("{from}/")) {
        return Err("invalid_folder".into());
    }
    let _guard = WRITE_LOCK.lock().map_err(|_| "unavailable".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    let mut registry = read_folder_registry(root)?;
    let mut changes = Vec::new();
    for path in document_files(root)? {
        let original = std::fs::read_to_string(&path).map_err(|_| "unavailable".to_string())?;
        let (head, _) = parse(&original)?;
        let expected_name = format!("{}.md", head.id);
        if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
            return Err("invalid_document".into());
        }
        let suffix = head
            .folder
            .strip_prefix(from)
            .filter(|suffix| suffix.is_empty() || suffix.starts_with('/'));
        if let Some(suffix) = suffix {
            let folder = format!("{to}{suffix}");
            let updated = document_with_folder(&original, &folder)?;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ZetUpdate {
    pub(crate) title: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) assets: Option<Vec<String>>,
    pub(crate) edges: Option<Vec<Edge>>,
}

pub(crate) fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

pub(crate) static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn update(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
    changes: ZetUpdate,
) -> Result<(), String> {
    update_checked(root, database, id, changes, None)
}

pub(crate) fn update_checked(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
    changes: ZetUpdate,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    let _guard = WRITE_LOCK
        .lock()
        .map_err(|_| "글을 고치지 못했습니다".to_string())?;
    update_inner(root, database, id, changes, expected_hash)
}

pub(crate) fn update_inner(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
    changes: ZetUpdate,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    use std::io::Write;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    if !valid_zet_id(id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    let path = root.join("zettels").join(format!("{id}.md"));
    let text = std::fs::read_to_string(&path).map_err(|_| "글을 읽지 못했습니다".to_string())?;
    if expected_hash.is_some_and(|expected| expected != content_hash(&text)) {
        return Err("이 글이 그사이 바뀌었습니다".into());
    }
    let (mut head, body) = parse(&text)?;
    if head.id != id {
        return Err("글 번호와 파일 이름이 다릅니다".into());
    }
    if let Some(value) = changes.title {
        head.title = value;
    }
    if let Some(value) = changes.summary {
        head.summary = value;
    }
    if let Some(value) = changes.status {
        head.status = value;
    }
    if let Some(value) = changes.assets {
        head.assets = value;
    }
    if let Some(value) = changes.edges {
        head.edges = value;
    }
    head.updated_at = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let content = render(&head, changes.body.as_deref().unwrap_or(body))?;
    let mut temporary = tempfile::NamedTempFile::new_in(root.join("zettels"))
        .map_err(|_| "글을 고치지 못했습니다".to_string())?;
    temporary
        .write_all(content.as_bytes())
        .map_err(|_| "글을 고치지 못했습니다".to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| "글을 고치지 못했습니다".to_string())?;
    if expected_hash.is_some() {
        let current =
            std::fs::read_to_string(&path).map_err(|_| "글을 읽지 못했습니다".to_string())?;
        if current != text {
            return Err("이 글이 그사이 바뀌었습니다".into());
        }
    }
    temporary
        .persist(&path)
        .map_err(|_| "글을 고치지 못했습니다".to_string())?;
    let mut conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "글 색인을 열지 못했습니다".to_string())?;
    let tx = conn
        .transaction()
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    tx.execute("INSERT OR REPLACE INTO archive_zets(id,title,summary,kind,status,created_at,updated_at,file_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", rusqlite::params![head.id,head.title,head.summary,head.kind,head.status,head.created_at,head.updated_at,format!("zettels/{id}.md")]).map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    tx.execute("DELETE FROM archive_zets_assets WHERE zet_id=?1", [id])
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    tx.execute("DELETE FROM archive_zets_edges WHERE zet_id=?1", [id])
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    for asset in &head.assets {
        tx.execute(
            "INSERT OR IGNORE INTO archive_zets_assets(zet_id,asset_id) VALUES (?1,?2)",
            rusqlite::params![id, asset],
        )
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    }
    for edge in &head.edges {
        tx.execute(
            "INSERT OR IGNORE INTO archive_zets_edges(zet_id,edge_type,target_id) VALUES (?1,?2,?3)",
            rusqlite::params![id, edge.kind, edge.target],
        )
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    }
    tx.commit()
        .map_err(|_| "글 색인을 갱신하지 못했습니다".to_string())?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkedZet {
    #[serde(rename = "type")]
    kind: String,
    id: String,
    title: Option<String>,
    zet_kind: Option<String>,
    status: Option<String>,
    missing: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkedAsset {
    id: String,
    original_file_name: Option<String>,
    deleted: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LinkedReceipt {
    id: String,
    created_at: String,
    tool: String,
    model: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ZetLinked {
    assets: Vec<LinkedAsset>,
    outgoing: Vec<LinkedZet>,
    incoming: Vec<LinkedZet>,
    receipts: Vec<LinkedReceipt>,
    chats: Vec<crate::archive_receipt::ReferencingChat>,
}

pub(crate) fn linked(
    root: &std::path::Path,
    database: &std::path::Path,
    id: &str,
) -> Result<ZetLinked, String> {
    if !valid_zet_id(id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    let conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "연결을 읽지 못했습니다".to_string())?;
    let edges = |sql: &str| -> Result<Vec<LinkedZet>, String> {
        let mut stmt = conn
            .prepare(sql)
            .map_err(|_| "연결을 읽지 못했습니다".to_string())?;
        let rows = stmt
            .query_map([id], |r| {
                let linked_id: String = r.get(1)?;
                let file_exists = root
                    .join("zettels")
                    .join(format!("{linked_id}.md"))
                    .is_file();
                let title: Option<String> = if file_exists { r.get(2)? } else { None };
                let missing = title.is_none();
                Ok(LinkedZet {
                    kind: r.get(0)?,
                    id: linked_id,
                    missing,
                    title,
                    zet_kind: if missing { None } else { r.get(3)? },
                    status: if missing { None } else { r.get(4)? },
                })
            })
            .map_err(|_| "연결을 읽지 못했습니다".to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "연결을 읽지 못했습니다".into())
    };
    let mut stmt = conn.prepare("SELECT a.asset_id, s.original_file_name, EXISTS(SELECT 1 FROM source_asset_deletions d WHERE d.asset_id=s.id) FROM archive_zets_assets a LEFT JOIN momso_source_assets s ON s.content_sha256=substr(a.asset_id,8) WHERE a.zet_id=?1 ORDER BY a.asset_id").map_err(|_| "연결을 읽지 못했습니다".to_string())?;
    let assets = stmt
        .query_map([id], |r| {
            Ok(LinkedAsset {
                id: r.get(0)?,
                original_file_name: r.get(1)?,
                deleted: r.get(2)?,
            })
        })
        .map_err(|_| "연결을 읽지 못했습니다".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "연결을 읽지 못했습니다".to_string())?;
    let mut stmt = conn.prepare("SELECT r.id,r.created_at,r.tool,r.model FROM archive_receipts_wrote w JOIN archive_receipts r ON r.id=w.receipt_id WHERE w.reference_id=?1 ORDER BY r.id").map_err(|_| "연결을 읽지 못했습니다".to_string())?;
    let receipts = stmt
        .query_map([id], |r| {
            Ok(LinkedReceipt {
                id: r.get(0)?,
                created_at: r.get(1)?,
                tool: r.get(2)?,
                model: r.get(3)?,
            })
        })
        .map_err(|_| "연결을 읽지 못했습니다".to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "연결을 읽지 못했습니다".to_string())?;
    let chats = crate::archive_receipt::chats_for_reference(root, database, id)?;
    Ok(ZetLinked { assets, outgoing: edges("SELECT e.edge_type,e.target_id,z.title,z.kind,z.status FROM archive_zets_edges e LEFT JOIN archive_zets z ON z.id=e.target_id WHERE e.zet_id=?1 ORDER BY e.edge_type,e.target_id")?, incoming: edges("SELECT e.edge_type,e.zet_id,z.title,z.kind,z.status FROM archive_zets_edges e LEFT JOIN archive_zets z ON z.id=e.zet_id WHERE e.target_id=?1 ORDER BY e.edge_type,e.zet_id")?, receipts, chats })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folder_validation_and_legacy_document() {
        assert!(validate_folder("수업 기록/2026-11").is_ok());
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("zettels"), "not a directory").unwrap();
        assert!(folders(temp.path()).is_err());
        let legacy = "---\nid: zet_20261115_143052\ntitle: Lesson\nsummary: Notes\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\nBody\n";
        let (mut head, body) = parse(legacy).unwrap();
        assert_eq!(head.folder, "");
        assert_eq!(render(&head, body).unwrap(), legacy);
        head.folder = "Classes/2026-11".into();
        let updated = render(&head, body).unwrap();
        assert_eq!(parse(&updated).unwrap().0.folder, "Classes/2026-11");
        for folder in [
            "/Classes",
            "Classes/",
            "Classes//Notes",
            "Classes/../Notes",
            " Classes",
            "Classes ",
            "Classes\\Notes",
        ] {
            assert!(validate_folder(folder).is_err(), "{folder}");
        }
    }
    #[test]
    fn legacy_move_preserves_body_links_and_saved_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = create(
            &root,
            &db,
            ZetCreate {
                title: "Lesson".into(),
                summary: "Notes".into(),
                kind: "note".into(),
                body: "Body [link](https://example.com)\n".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let path = root.join("zettels").join(format!("{id}.md"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\nfolder:"));
        crate::archive::reindex(&root, &db).unwrap();
        let mut entries = list(&db, None, None).unwrap();
        add_folders(&root, &mut entries).unwrap();
        assert_eq!(
            entries.iter().find(|entry| entry.id == id).unwrap().folder,
            ""
        );
        let issues = crate::archive::check(&root, temp.path()).unwrap();
        assert!(
            issues.is_empty(),
            "{}",
            serde_json::to_string(&issues).unwrap()
        );
        crate::archive::save(&root, temp.path(), &db).unwrap();
        set_folder(&root, &db, &id, "Classes").unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after.replace("folder: Classes\n", ""), text);
        assert_eq!(parse(&after).unwrap().0.id, id);
        crate::archive::save(&root, temp.path(), &db).unwrap();
        let (git, _) = crate::archive::storage_tool(temp.path()).unwrap();
        let previous = crate::archive::run_storage_tool(
            &git,
            &root,
            &["show", &format!("HEAD~1:zettels/{id}.md")],
        )
        .unwrap();
        assert_eq!(previous, text);
    }
    #[test]
    fn renamed_folder_from_document_without_registry_stays_empty_after_move() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = "zet_20261115_143052";
        let text = "---\nid: zet_20261115_143052\ntitle: Lesson\nsummary: Notes\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\nfolder: Classes/Manual\n---\nBody\n";
        std::fs::write(root.join("zettels").join(format!("{id}.md")), text).unwrap();
        rename_folder(&root, &db, "Classes", "Lessons").unwrap();
        set_folder(&root, &db, id, "").unwrap();
        assert_eq!(folders(&root).unwrap(), vec!["Lessons", "Lessons/Manual"]);
    }
    #[test]
    fn folder_create_move_rename_and_save() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        create_folder(&root, &db, "Classes/Empty").unwrap();
        let input = ZetCreate {
            title: "Lesson".into(),
            summary: "Notes".into(),
            kind: "note".into(),
            body: "Body".into(),
            assets: vec![],
            edges: vec![],
        };
        let id = create_in_folder(&root, &db, input, "Classes/November").unwrap();
        let other = create_in_folder(
            &root,
            &db,
            ZetCreate {
                title: "Second".into(),
                summary: "Notes".into(),
                kind: "note".into(),
                body: "Other body".into(),
                assets: vec![],
                edges: vec![],
            },
            "Classes/November",
        )
        .unwrap();
        let path = root.join("zettels").join(format!("{id}.md"));
        assert_eq!(
            parse(&std::fs::read_to_string(&path).unwrap())
                .unwrap()
                .0
                .folder,
            "Classes/November"
        );
        set_folder(&root, &db, &id, "Classes/Empty").unwrap();
        rename_folder(&root, &db, "Classes", "Lessons").unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        let (head, body) = parse(&saved).unwrap();
        assert_eq!(head.folder, "Lessons/Empty");
        assert_eq!(body, "Body");
        let other_text =
            std::fs::read_to_string(root.join("zettels").join(format!("{other}.md"))).unwrap();
        assert_eq!(parse(&other_text).unwrap().0.folder, "Lessons/November");
        assert_eq!(
            folders(&root).unwrap(),
            vec!["Lessons", "Lessons/Empty", "Lessons/November"]
        );
        assert!(set_folder(&root, &db, &id, "../Bad").is_err());
        assert!(path.exists());
        let issues = crate::archive::check(&root, temp.path()).unwrap();
        assert!(
            issues.is_empty(),
            "{}",
            serde_json::to_string(&issues).unwrap()
        );
        crate::archive::save(&root, temp.path(), &db).unwrap();
        let (git, _) = crate::archive::storage_tool(temp.path()).unwrap();
        let bare = temp.path().join("remote.git");
        crate::archive::run_storage_tool(
            &git,
            temp.path(),
            &["init", "--bare", bare.to_str().unwrap()],
        )
        .unwrap();
        crate::archive::run_storage_tool(
            &git,
            &root,
            &["push", bare.to_str().unwrap(), "HEAD:refs/heads/main"],
        )
        .unwrap();
        let stored = crate::archive::run_storage_tool(
            &git,
            temp.path(),
            &[
                "--git-dir",
                bare.to_str().unwrap(),
                "show",
                "main:zettels/.folders.json",
            ],
        )
        .unwrap();
        assert!(stored.contains("Lessons/Empty"));
        let clone = temp.path().join("new-pc");
        crate::archive::run_storage_tool(
            &git,
            temp.path(),
            &[
                "clone",
                "--branch",
                "main",
                bare.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        )
        .unwrap();
        let received =
            std::fs::read_to_string(clone.join("zettels").join(format!("{id}.md"))).unwrap();
        assert_eq!(parse(&received).unwrap().0.folder, "Lessons/Empty");
        assert_eq!(
            folders(&clone).unwrap(),
            vec!["Lessons", "Lessons/Empty", "Lessons/November"]
        );
        crate::archive::reindex(&root, &db).unwrap();
        let mut entries = list(&db, None, None).unwrap();
        add_folders(&root, &mut entries).unwrap();
        assert_eq!(
            entries.iter().find(|entry| entry.id == id).unwrap().folder,
            "Lessons/Empty"
        );
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.id == other)
                .unwrap()
                .folder,
            "Lessons/November"
        );
    }
    #[test]
    fn korean_and_typed_edge_round_trip() {
        let head = ZetHead {
            id: "zet_20261115_143052".into(),
            title: "요가 수업".into(),
            summary: "골반 정렬".into(),
            kind: "summary".into(),
            status: "draft".into(),
            created_at: "2026-11-15T14:30:52+09:00".into(),
            updated_at: "2026-11-15T14:30:52+09:00".into(),
            assets: vec![],
            edges: vec![Edge {
                kind: "summarizes".into(),
                target: "zet_20261115_143055".into(),
            }],
            folder: String::new(),
            original_header: None,
        };
        let rendered = render(&head, "한글 본문\n").unwrap();
        let (parsed, body) = parse(&rendered).unwrap();
        let mut comparable = parsed.clone();
        comparable.original_header = None;
        assert_eq!(comparable, head);
        assert_eq!(render(&parsed, body).unwrap(), rendered);
        let mut invalid = head;
        invalid.edges[0].kind = "unknown".into();
        assert!(validate(&invalid).is_err());
    }

    #[test]
    fn unchanged_hand_written_header_round_trips_byte_for_byte() {
        let text = "---\r\nid: zet_20261115_143052\r\ntitle: '한글 제목'\r\nsummary: \"요약\"\r\nkind: note\r\nstatus: draft\r\ncreated_at: '2026-11-15T14:30:52+09:00'\r\nupdated_at: '2026-11-15T14:30:52+09:00'\r\nassets: []\r\nedges: []\r\n---\r\n본문\r\n";
        let (head, body) = parse(text).unwrap();
        assert_eq!(render(&head, body).unwrap(), text);
    }

    #[test]
    fn creates_two_distinct_files_and_indexes_them() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let input = || ZetCreate {
            title: "한글 수업".into(),
            summary: "요약".into(),
            kind: "note".into(),
            body: "한글 본문".into(),
            assets: vec![],
            edges: vec![],
        };
        let first = create(&root, &db, input()).unwrap();
        let second = create(&root, &db, input()).unwrap();
        assert_ne!(first, second);
        let mut third_input = input();
        third_input.edges = vec![Edge {
            kind: "summarizes".into(),
            target: first.clone(),
        }];
        let third = create(&root, &db, third_input).unwrap();
        let connection = crate::storage::open_writable_connection(&db).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM archive_zets", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 3);
        assert!(root.join("zettels").join(format!("{first}.md")).is_file());
        assert!(root.join("zettels").join(format!("{second}.md")).is_file());
        let links = linked(&root, &db, &third).unwrap();
        assert_eq!(links.outgoing.len(), 1);
        assert_eq!(links.outgoing[0].kind, "summarizes");
        assert_eq!(links.outgoing[0].id, first);
        let file = root.join("zettels").join(format!("{third}.md"));
        let before_text = std::fs::read_to_string(&file).unwrap();
        let (before, _) = parse(&before_text).unwrap();
        update(
            &root,
            &db,
            &third,
            ZetUpdate {
                title: Some("고친 제목".into()),
                summary: None,
                body: Some("고친 본문".into()),
                status: Some("confirmed".into()),
                assets: None,
                edges: None,
            },
        )
        .unwrap();
        let after_text = std::fs::read_to_string(&file).unwrap();
        let (after, after_body) = parse(&after_text).unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(after.title, "고친 제목");
        assert_eq!(after.status, "confirmed");
        assert_eq!(after_body, "고친 본문");
    }
}
