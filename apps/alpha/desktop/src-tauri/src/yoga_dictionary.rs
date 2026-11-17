use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

// This is the one place where the preliminary file layout is selected.
pub(crate) const DIRECTORY: &str = "dictionary";
const BASE: &str = "yoga.json";
const USER: &str = "my-terms.json";
const PEOPLE: &str = "people.json";
const BASE_CONTENT: &str = include_str!("../bundled-dictionary/yoga.json");
const EMPTY_USER: &str = "{\n  \"terms\": []\n}\n";
const EMPTY_PEOPLE: &str = "{\n  \"people\": []\n}\n";
// Each later bundle must retain earlier exact bundle bytes here for safe upgrade.
const PREVIOUS_BUNDLES: &[&str] = &[
    include_str!("../bundled-dictionary/yoga-v1.json"),
    include_str!("../bundled-dictionary/yoga-v2.json"),
];

pub(crate) fn known_bundle_version(content: &str) -> Option<u64> {
    PREVIOUS_BUNDLES
        .iter()
        .copied()
        .chain(std::iter::once(BASE_CONTENT))
        .find(|known| *known == content)
        .and_then(|known| serde_json::from_str::<serde_json::Value>(known).ok())
        .and_then(|value| {
            value
                .get("bundleVersion")
                .and_then(serde_json::Value::as_u64)
        })
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Term {
    id: String,
    #[serde(default = "default_true")]
    enabled: bool,
    sanskrit: String,
    #[serde(default)]
    roman: String,
    #[serde(default)]
    language: String,
    korean: Vec<String>,
    korean_names: Vec<String>,
    meaning: String,
    sources: Vec<String>,
    #[serde(default)]
    category: String,
    #[serde(default)]
    verified_sources: Vec<String>,
    #[serde(default)]
    unverified_sources: Vec<String>,
    #[serde(default)]
    source_assessment: String,
    #[serde(default)]
    variants: Vec<Variant>,
    #[serde(default)]
    ambiguity_reason: Option<String>,
}
fn default_true() -> bool {
    true
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Variant {
    text: String,
    origin: String,
    #[serde(default)]
    label: String,
    recognition_only: bool,
    sources: Vec<String>,
    #[serde(default)]
    unverified_sources: Vec<String>,
    #[serde(default)]
    generated_from: Vec<String>,
    #[serde(default)]
    requires_confirmation: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bundle {
    bundle_version: u32,
    review_status: String,
    terms: Vec<Term>,
    #[serde(default)]
    fragments: Vec<Fragment>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Fragment {
    id: String,
    roman: String,
    meaning: String,
    variants: Vec<FragmentVariant>,
    sources: Vec<String>,
    review_status: String,
}
#[derive(Clone, Deserialize, Serialize)]
struct FragmentVariant {
    text: String,
    origin: String,
    sources: Vec<String>,
    label: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserEntry {
    term: Term,
    base_meaning: Option<String>,
}
#[derive(Deserialize)]
struct UserTerms {
    terms: Vec<UserEntry>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ListedTerm {
    #[serde(flatten)]
    term: Term,
    origin: &'static str,
    user_kind: Option<&'static str>,
    can_delete: bool,
    source_file: String,
    conflict: Option<&'static str>,
    search_only: Vec<Variant>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListedPerson {
    id: Option<String>,
    name: String,
    aliases: Vec<String>,
    confirmed: bool,
    origin: &'static str,
    source_file: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DictionaryList {
    bundle_version: u32,
    review_status: String,
    base_status: &'static str,
    base_conflicts: Vec<BaseConflict>,
    terms: Vec<ListedTerm>,
    fragments: Vec<Fragment>,
    people: Vec<ListedPerson>,
    people_status: &'static str,
    user_revision: String,
    people_revision: String,
}
fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(crate) fn user_revision(root: &Path) -> Result<String, String> {
    let dir = root.join(DIRECTORY);
    let meta = fs::symlink_metadata(&dir).map_err(|_| "dictionary_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    Ok(revision(&read_safe(&dir, USER)?))
}
pub(crate) fn term_label(root: &Path, id: &str) -> Result<String, String> {
    list(root)?
        .terms
        .into_iter()
        .find(|item| item.term.id == id)
        .and_then(|item| item.term.korean.into_iter().next())
        .ok_or("not_found".into())
}

pub(crate) fn delete_preview(root: &Path, id: &str) -> Result<(String, bool, String), String> {
    let dictionary = list(root)?;
    let item = dictionary
        .terms
        .iter()
        .find(|item| item.term.id == id)
        .ok_or("not_found")?;
    let label = item
        .term
        .korean
        .first()
        .cloned()
        .unwrap_or_else(|| item.term.id.clone());
    Ok((
        label
            .chars()
            .filter(|c| !c.is_control())
            .take(100)
            .collect(),
        item.can_delete,
        dictionary.user_revision,
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BaseConflict {
    id: String,
    old_bundle_version: u32,
    old_meaning: String,
    new_meaning: Option<String>,
    reason: &'static str,
}

fn base_conflicts(old: &Bundle, current: &Bundle) -> Vec<BaseConflict> {
    let new_terms: HashMap<_, _> = current.terms.iter().map(|t| (&t.id, t)).collect();
    old.terms
        .iter()
        .filter_map(|previous| {
            let current_term = new_terms.get(&previous.id);
            let reason = match current_term {
                None => "base_removed",
                Some(term) if term.meaning != previous.meaning => "base_changed",
                Some(_) => return None,
            };
            Some(BaseConflict {
                id: previous.id.clone(),
                old_bundle_version: old.bundle_version,
                old_meaning: previous.meaning.clone(),
                new_meaning: current_term.map(|t| t.meaning.clone()),
                reason,
            })
        })
        .collect()
}

fn files() -> [(&'static str, &'static str); 3] {
    [
        (BASE, BASE_CONTENT),
        (USER, EMPTY_USER),
        (PEOPLE, EMPTY_PEOPLE),
    ]
}
pub(crate) fn paths() -> Vec<String> {
    files()
        .iter()
        .map(|(name, _)| format!("{DIRECTORY}/{name}"))
        .collect()
}
pub(crate) fn is_path(path: &str) -> bool {
    paths().iter().any(|allowed| allowed == path)
}

fn safe_directory(root: &Path) -> Result<std::path::PathBuf, String> {
    let dir = root.join(DIRECTORY);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && !crate::archive_location::is_link_or_reparse(&meta) => {}
        Ok(_) => return Err("dictionary_unsafe".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&dir).map_err(|_| "dictionary_unavailable")?
        }
        Err(_) => return Err("dictionary_unavailable".into()),
    }
    Ok(dir)
}

pub(crate) fn initialize(root: &Path) -> Result<(), String> {
    let dir = safe_directory(root)?;
    let mut created = Vec::new();
    for (name, content) in files() {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(name))
        {
            Ok(mut f) => {
                f.write_all(content.as_bytes())
                    .map_err(|_| "dictionary_unavailable")?;
                created.push((name, content));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta =
                    fs::symlink_metadata(dir.join(name)).map_err(|_| "dictionary_unavailable")?;
                if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
                    return Err("dictionary_unsafe".into());
                }
            }
            Err(_) => return Err("dictionary_unavailable".into()),
        }
    }
    if !created.is_empty() {
        if let Ok((git, _)) = crate::archive::storage_tool(root) {
            if crate::archive::run_storage_tool(&git, root, &["rev-parse", "--verify", "HEAD"])
                .is_ok()
            {
                let mut paths = Vec::new();
                for (name, content) in &created {
                    let relative = format!("{DIRECTORY}/{name}");
                    let tracked = crate::archive::run_storage_tool(
                        &git,
                        root,
                        &["ls-files", "--stage", "--", &relative],
                    )
                    .map_err(|_| "dictionary_unavailable")?;
                    if tracked.trim().is_empty()
                        && fs::read(dir.join(name)).is_ok_and(|v| v == content.as_bytes())
                    {
                        paths.push(relative);
                    }
                }
                if !paths.is_empty() {
                    let mut add = vec!["add", "--"];
                    add.extend(paths.iter().map(String::as_str));
                    crate::archive::run_storage_tool(&git, root, &add)
                        .map_err(|_| "dictionary_save_failed")?;
                    let mut commit = vec![
                        "-c",
                        "user.name=momso",
                        "-c",
                        "user.email=archive@momso.local",
                        "commit",
                        "--only",
                        "-m",
                        "몸소 업데이트: 기본 용어 사전 설치",
                        "--",
                    ];
                    commit.extend(paths.iter().map(String::as_str));
                    crate::archive::run_storage_tool(&git, root, &commit)
                        .map_err(|_| "dictionary_save_failed")?;
                }
            }
        }
    }
    #[cfg(windows)]
    {
        let current = fs::read(dir.join(BASE)).map_err(|_| "dictionary_unavailable")?;
        if let Some(old) = PREVIOUS_BUNDLES
            .iter()
            .find(|old| current == old.as_bytes())
        {
            let tool = crate::archive::storage_tool(root).ok().map(|v| v.0);
            let has_history = tool.as_ref().is_some_and(|git| {
                crate::archive::run_storage_tool(git, root, &["rev-parse", "--verify", "HEAD"])
                    .is_ok()
            });
            let relative = format!("{DIRECTORY}/{BASE}");
            let clean = tool.as_ref().is_some_and(|git| {
                has_history
                    && crate::archive::run_storage_tool(
                        git,
                        root,
                        &["status", "--porcelain", "--", &relative],
                    )
                    .is_ok_and(|v| v.trim().is_empty())
                    && crate::archive::run_storage_tool(
                        git,
                        root,
                        &["show", &format!("HEAD:{relative}")],
                    )
                    .is_ok_and(|v| v.as_bytes() == current)
            });
            if !has_history || clean {
                // The exclusive open in replace_known_base rechecks the bytes.
                if replace_known_base(root, old.as_bytes(), BASE_CONTENT.as_bytes())? && clean {
                    let git = tool.as_ref().ok_or("dictionary_unavailable")?;
                    if fs::read(dir.join(BASE)).map_err(|_| "dictionary_unavailable")?
                        != BASE_CONTENT.as_bytes()
                        || crate::archive::run_storage_tool(
                            git,
                            root,
                            &["diff", "--cached", "--quiet", "--", &relative],
                        )
                        .is_err()
                    {
                        return Err("dictionary_changed".into());
                    }
                    if crate::archive::run_storage_tool(
                        git,
                        root,
                        &[
                            "-c",
                            "user.name=momso",
                            "-c",
                            "user.email=archive@momso.local",
                            "commit",
                            "--only",
                            "-m",
                            "몸소 업데이트: 기본 용어 사전 갱신",
                            "--",
                            &relative,
                        ],
                    )
                    .is_err()
                    {
                        if fs::read(dir.join(BASE)).ok().as_deref() == Some(BASE_CONTENT.as_bytes())
                        {
                            fs::write(dir.join(BASE), old.as_bytes())
                                .map_err(|_| "dictionary_restore_failed")?;
                        }
                        return Err("dictionary_save_failed".into());
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn defaults_unchanged(root: &Path) -> bool {
    let dir = root.join(DIRECTORY);
    let Ok(meta) = fs::symlink_metadata(&dir) else {
        return false;
    };
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return false;
    }
    let Ok(entries) = fs::read_dir(&dir) else {
        return false;
    };
    let mut count = 0;
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let Some((_, expected)) = files().into_iter().find(|(n, _)| entry.file_name() == *n) else {
            return false;
        };
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if !meta.is_file()
            || crate::archive_location::is_link_or_reparse(&meta)
            || !fs::read(entry.path()).is_ok_and(|b| b == expected.as_bytes())
        {
            return false;
        }
        count += 1;
    }
    count == files().len()
}

fn dictionary_io_error(error: std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "dictionary_missing",
        std::io::ErrorKind::PermissionDenied => "dictionary_permission_denied",
        std::io::ErrorKind::StorageFull => "dictionary_disk_full",
        _ => "dictionary_unavailable",
    }
}

fn read_safe(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = dir.join(name);
    let meta = fs::symlink_metadata(&path).map_err(dictionary_io_error)?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    fs::read(path).map_err(|error| dictionary_io_error(error).into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TermEdit {
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) add_variants: Vec<String>,
    #[serde(default)]
    pub(crate) remove_variants: Vec<String>,
    pub(crate) enabled: Option<bool>,
    pub(crate) meaning: Option<String>,
    pub(crate) category: Option<String>,
    pub(crate) expected_revision: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PersonEdit {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) aliases: Vec<String>,
    #[serde(default)]
    pub(crate) confirmed_by_user: bool,
    pub(crate) expected_revision: String,
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max
        && !value.chars().any(|c| c.is_control())
}
fn safe_update(
    root: &Path,
    file: &str,
    expected: &str,
    mutate: impl FnOnce(&mut serde_json::Value) -> Result<String, String>,
) -> Result<String, String> {
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "dictionary_busy")?;
    let dir = root.join(DIRECTORY);
    let meta = fs::symlink_metadata(&dir).map_err(dictionary_io_error)?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    let path = dir.join(file);
    let meta = fs::symlink_metadata(&path).map_err(dictionary_io_error)?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x1 | 0x4);
    }
    let mut old_file = options.open(&path).map_err(|error| {
        if cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33)) {
            "dictionary_busy"
        } else {
            dictionary_io_error(error)
        }
    })?;
    let mut old = Vec::new();
    old_file
        .read_to_end(&mut old)
        .map_err(dictionary_io_error)?;
    if revision(&old) != expected {
        return Err("dictionary_changed".into());
    }
    let mut data: serde_json::Value =
        serde_json::from_slice(&old).map_err(|_| "dictionary_invalid")?;
    let id = mutate(&mut data)?;
    let mut bytes = serde_json::to_vec_pretty(&data).map_err(|_| "dictionary_invalid")?;
    bytes.push(b'\n');
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(dictionary_io_error)?;
    temp.write_all(&bytes)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(dictionary_io_error)?;
    if read_safe(&dir, file)? != old {
        return Err("dictionary_changed".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            ReplaceFileW, SetFileAttributesW, FILE_ATTRIBUTE_NORMAL,
        };
        let temp_path = temp.into_temp_path();
        let target: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let replacement: Vec<u16> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Both paths are terminated, owned here, and the target handle denies other writers.
        let ok = unsafe {
            SetFileAttributesW(replacement.as_ptr(), FILE_ATTRIBUTE_NORMAL) != 0
                && ReplaceFileW(
                    target.as_ptr(),
                    replacement.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                ) != 0
        };
        if !ok {
            return Err("dictionary_unavailable".into());
        }
    }
    #[cfg(not(windows))]
    temp.persist(&path).map_err(|_| "dictionary_unavailable")?;
    drop(old_file);
    Ok(id)
}

pub(crate) fn edit_term(root: &Path, edit: TermEdit) -> Result<String, String> {
    if edit.add_variants.len() > 64
        || edit.remove_variants.len() > 64
        || edit
            .add_variants
            .iter()
            .chain(&edit.remove_variants)
            .any(|v| !valid_text(v, 100))
        || edit
            .meaning
            .as_ref()
            .is_some_and(|v| v.chars().count() > 500)
        || edit.category.as_ref().is_some_and(|v| !valid_text(v, 80))
    {
        return Err("invalid_input".into());
    }
    let _ = user_revision(root)?;
    let base: serde_json::Value = serde_json::from_slice(&read_safe(&root.join(DIRECTORY), BASE)?)
        .map_err(|_| "dictionary_invalid")?;
    let expected = edit.expected_revision.clone();
    safe_update(root, USER, &expected, move |data| {
        let entries = data
            .get_mut("terms")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("dictionary_invalid")?;
        let id = edit
            .id
            .clone()
            .unwrap_or_else(|| format!("user-{}", uuid::Uuid::new_v4()));
        if id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid_input".into());
        }
        let matches: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.pointer("/term/id").and_then(serde_json::Value::as_str) == Some(&id))
            .map(|(i, _)| i)
            .collect();
        if matches.len() > 1 {
            return Err("dictionary_duplicate".into());
        }
        let index = if let Some(index) = matches.first() {
            *index
        } else {
            let (term, baseline) = if edit.id.is_some() {
                let term = base
                    .get("terms")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|t| t.get("id").and_then(serde_json::Value::as_str) == Some(&id))
                    })
                    .ok_or("not_found")?
                    .clone();
                let baseline = term
                    .get("meaning")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                (term, baseline)
            } else {
                if edit.add_variants.is_empty() {
                    return Err("invalid_input".into());
                }
                (
                    serde_json::json!({"id":id,"sanskrit":"","korean":[],"koreanNames":[],"meaning":"","sources":[],"category":"사용자","variants":[]}),
                    serde_json::Value::Null,
                )
            };
            entries.push(serde_json::json!({"term":term,"baseMeaning":baseline}));
            entries.len() - 1
        };
        let term = entries[index]
            .get_mut("term")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("dictionary_invalid")?;
        if let Some(enabled) = edit.enabled {
            term.insert("enabled".into(), enabled.into());
        }
        if let Some(meaning) = edit.meaning {
            term.insert("meaning".into(), meaning.into());
        }
        if let Some(category) = edit.category {
            term.insert("category".into(), category.trim().into());
        }
        let variants = term
            .entry("variants")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or("dictionary_invalid")?;
        variants.retain(|v| {
            !v.get("text")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|s| edit.remove_variants.iter().any(|r| r.trim() == s))
        });
        for value in &edit.add_variants {
            let value = value.trim();
            if !variants
                .iter()
                .any(|v| v.get("text").and_then(serde_json::Value::as_str) == Some(value))
            {
                variants.push(serde_json::json!({"text":value,"origin":"user","label":"내가 추가","recognitionOnly":false,"sources":[],"requiresConfirmation":false}));
            }
        }
        let korean = term
            .entry("korean")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or("dictionary_invalid")?;
        korean.retain(|v| {
            !v.as_str()
                .is_some_and(|s| edit.remove_variants.iter().any(|r| r.trim() == s))
        });
        for value in &edit.add_variants {
            let value = value.trim();
            if !korean.iter().any(|v| v.as_str() == Some(value)) {
                korean.push(value.into());
            }
        }
        Ok(id)
    })
}

fn deletable_id(id: &str) -> bool {
    id.strip_prefix("user-")
        .is_some_and(|value| uuid::Uuid::parse_str(value).is_ok())
}

pub(crate) fn delete_term(root: &Path, id: &str, expected: &str) -> Result<&'static str, String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("invalid_input".into());
    }
    let id = id.to_owned();
    let result = safe_update(root, USER, expected, move |data| {
        let dir = root.join(DIRECTORY);
        let base: Bundle =
            serde_json::from_slice(&read_safe(&dir, BASE)?).map_err(|_| "dictionary_invalid")?;
        if base.terms.iter().any(|term| term.id == id)
            || PREVIOUS_BUNDLES.iter().any(|source| {
                serde_json::from_str::<Bundle>(source)
                    .map_or(true, |bundle| bundle.terms.iter().any(|term| term.id == id))
            })
        {
            return Err("base_term".into());
        }
        let entries = data
            .get_mut("terms")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("dictionary_invalid")?;
        let matches: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry
                    .pointer("/term/id")
                    .and_then(serde_json::Value::as_str)
                    == Some(&id)
            })
            .map(|(index, _)| index)
            .collect();
        if matches.len() > 1 {
            return Err("dictionary_duplicate".into());
        }
        let index = *matches.first().ok_or("not_found")?;
        if !deletable_id(&id)
            || entries[index]
                .get("baseMeaning")
                .is_some_and(|value| !value.is_null())
        {
            return Err("base_term".into());
        }
        entries.remove(index);
        Ok(id)
    });
    match result {
        Ok(_) => Ok("deleted"),
        Err(error) if error == "base_term" => Ok("base_term"),
        Err(error) => Err(error),
    }
}

fn person_id(entry: &serde_json::Value, index: usize) -> String {
    entry
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("legacy-{index}"))
}
fn check_person_ids(people: &[serde_json::Value]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    if people
        .iter()
        .enumerate()
        .any(|(i, v)| !seen.insert(person_id(v, i)))
    {
        return Err("dictionary_duplicate".into());
    }
    Ok(())
}
pub(crate) fn edit_person(root: &Path, edit: PersonEdit) -> Result<String, String> {
    if !valid_text(&edit.name, 100)
        || edit.aliases.len() > 64
        || edit.aliases.iter().any(|v| !valid_text(v, 100))
    {
        return Err("invalid_input".into());
    }
    let expected = edit.expected_revision.clone();
    safe_update(root, PEOPLE, &expected, move |data| {
        let people = data
            .get_mut("people")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("dictionary_invalid")?;
        check_person_ids(people)?;
        let index = edit
            .id
            .as_ref()
            .map(|id| {
                people
                    .iter()
                    .enumerate()
                    .find(|(i, v)| person_id(v, *i) == *id)
                    .map(|(i, _)| i)
                    .ok_or("not_found")
            })
            .transpose()?;
        let id = match index {
            Some(i) => {
                if people[i]
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .is_some()
                {
                    person_id(&people[i], i)
                } else {
                    format!("person-{}", uuid::Uuid::new_v4())
                }
            }
            None => format!("person-{}", uuid::Uuid::new_v4()),
        };
        let mut value = index
            .and_then(|i| people[i].as_object().cloned())
            .unwrap_or_default();
        value.insert("id".into(), id.clone().into());
        value.insert("name".into(), edit.name.trim().into());
        value.insert(
            "aliases".into(),
            serde_json::json!(edit.aliases.iter().map(|v| v.trim()).collect::<Vec<_>>()),
        );
        value.insert("confirmed".into(), edit.confirmed_by_user.into());
        if let Some(i) = index {
            people[i] = value.into();
        } else {
            people.push(value.into());
        }
        check_person_ids(people)?;
        Ok(id)
    })
}
pub(crate) fn delete_person(root: &Path, id: &str, expected: &str) -> Result<(), String> {
    let wanted = id.to_owned();
    safe_update(root, PEOPLE, expected, move |data| {
        let people = data
            .get_mut("people")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or("dictionary_invalid")?;
        check_person_ids(people)?;
        let index = people
            .iter()
            .enumerate()
            .find(|(i, v)| person_id(v, *i) == wanted)
            .map(|(i, _)| i)
            .ok_or("not_found")?;
        people.remove(index);
        check_person_ids(people)?;
        Ok(wanted)
    })
    .map(|_| ())
}

pub(crate) fn list(root: &Path) -> Result<DictionaryList, String> {
    let dir = root.join(DIRECTORY);
    let meta = fs::symlink_metadata(&dir).map_err(|_| "dictionary_unavailable")?;
    if !meta.is_dir() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    let base_bytes = read_safe(&dir, BASE)?;
    let base_status = if base_bytes == BASE_CONTENT.as_bytes() {
        "current"
    } else {
        "modified_or_previous"
    };
    let bundle: Bundle = serde_json::from_slice(&base_bytes).map_err(|_| "dictionary_invalid")?;
    let mut conflicts = Vec::new();
    let mut previous_ids = std::collections::HashSet::new();
    for previous in PREVIOUS_BUNDLES {
        let old: Bundle = serde_json::from_str(previous).map_err(|_| "dictionary_invalid")?;
        conflicts.extend(base_conflicts(&old, &bundle));
        previous_ids.extend(old.terms.into_iter().map(|term| term.id));
    }
    let user_bytes = read_safe(&dir, USER)?;
    let user: UserTerms = serde_json::from_slice(&user_bytes).map_err(|_| "dictionary_invalid")?;
    let mut base: HashMap<String, Term> = HashMap::new();
    for term in &bundle.terms {
        if base.insert(term.id.clone(), term.clone()).is_some() {
            return Err("dictionary_duplicate".into());
        }
    }
    let mut overrides = HashMap::new();
    for entry in user.terms {
        if overrides.insert(entry.term.id.clone(), entry).is_some() {
            return Err("dictionary_duplicate".into());
        }
    }
    let mut terms = Vec::new();
    for t in bundle.terms {
        if let Some(entry) = overrides.remove(&t.id) {
            let conflict = match entry.base_meaning.as_deref() {
                None => Some("base_unknown"),
                Some(m) if m != t.meaning => Some("base_changed"),
                _ => None,
            };
            terms.push(ListedTerm {
                term: entry.term,
                origin: "user",
                user_kind: Some("modified"),
                can_delete: false,
                source_file: format!("{DIRECTORY}/{USER}"),
                conflict,
                search_only: Vec::new(),
            });
        } else {
            terms.push(ListedTerm {
                term: t,
                origin: "default",
                user_kind: None,
                can_delete: false,
                source_file: format!("{DIRECTORY}/{BASE}"),
                conflict: None,
                search_only: Vec::new(),
            });
        }
    }
    let mut remaining: Vec<_> = overrides.into_values().collect();
    remaining.sort_by(|a, b| a.term.id.cmp(&b.term.id));
    for entry in remaining {
        let conflict = if entry.base_meaning.is_some() {
            Some("base_removed")
        } else {
            None
        };
        let can_delete = entry.base_meaning.is_none()
            && deletable_id(&entry.term.id)
            && !previous_ids.contains(&entry.term.id);
        terms.push(ListedTerm {
            term: entry.term,
            origin: "user",
            user_kind: Some(if conflict.is_some() || !can_delete {
                "modified"
            } else {
                "added"
            }),
            can_delete,
            source_file: format!("{DIRECTORY}/{USER}"),
            conflict,
            search_only: Vec::new(),
        });
    }
    // Compare the effective user and default terms, including recognition-only
    // spellings, before removing those spellings from the screen response.
    let mut owners: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for item in &terms {
        for variant in &item.term.variants {
            owners
                .entry(variant.text.trim().to_lowercase())
                .or_default()
                .insert(item.term.id.clone());
        }
    }
    for item in &mut terms {
        for variant in &mut item.term.variants {
            if item.term.ambiguity_reason.is_some()
                || owners
                    .get(&variant.text.trim().to_lowercase())
                    .is_some_and(|ids| ids.len() > 1)
            {
                variant.requires_confirmation = true;
            }
        }
        item.search_only = item
            .term
            .variants
            .iter()
            .filter(|v| v.recognition_only)
            .cloned()
            .collect();
        item.term.variants.retain(|v| {
            ["observed", "source_unverified", "user"].contains(&v.origin.as_str())
                && !v.recognition_only
        });
    }
    let people_bytes = read_safe(&dir, PEOPLE)?;
    let people_file: Option<serde_json::Value> = serde_json::from_slice(&people_bytes).ok();
    let entries = people_file
        .as_ref()
        .and_then(|v| v.get("people"))
        .and_then(serde_json::Value::as_array);
    let people_status = if entries.is_some() { "ok" } else { "invalid" };
    let empty = Vec::new();
    let entries = entries.unwrap_or(&empty);
    let mut people = Vec::with_capacity(entries.len());
    let mut person_ids = std::collections::HashSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let (id, name, aliases, confirmed) = if let Some(name) = entry.as_str() {
            (
                Some(format!("legacy-{index}")),
                name.to_owned(),
                Vec::new(),
                false,
            )
        } else {
            let obj = entry.as_object().ok_or("dictionary_invalid")?;
            let name = obj
                .get("name")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .ok_or("dictionary_invalid")?
                .to_owned();
            let aliases = match obj.get("aliases") {
                None => Vec::new(),
                Some(value) => {
                    serde_json::from_value(value.clone()).map_err(|_| "dictionary_invalid")?
                }
            };
            (
                Some(
                    obj.get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("legacy-{index}")),
                ),
                name,
                aliases,
                obj.get("confirmed")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            )
        };
        if !person_ids.insert(id.clone().ok_or("dictionary_invalid")?) {
            return Err("dictionary_duplicate".into());
        }
        people.push(ListedPerson {
            id,
            name,
            aliases,
            confirmed,
            origin: "user",
            source_file: format!("{DIRECTORY}/{PEOPLE}"),
        });
    }
    Ok(DictionaryList {
        bundle_version: bundle.bundle_version,
        review_status: bundle.review_status,
        base_status,
        base_conflicts: conflicts,
        terms,
        fragments: bundle.fragments,
        people,
        people_status,
        user_revision: revision(&user_bytes),
        people_revision: revision(&people_bytes),
    })
}

// Only previously bundled, byte-identical content can be updated. A durable
// backup is retained before writing; Windows denies concurrent open writers.
#[cfg(windows)]
fn replace_known_base(root: &Path, old: &[u8], new: &[u8]) -> Result<bool, String> {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = safe_directory(root)?;
    let path = dir.join(BASE);
    let meta = fs::symlink_metadata(&path).map_err(|_| "dictionary_unavailable")?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&path)
        .map_err(|_| "dictionary_busy")?;
    let mut current = Vec::new();
    f.read_to_end(&mut current)
        .map_err(|_| "dictionary_unavailable")?;
    if current != old {
        return Ok(false);
    }
    let backup = root
        .join(".git")
        .join(format!("momso-dictionary-{}", uuid::Uuid::new_v4()));
    let mut b = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)
        .map_err(|_| "dictionary_unavailable")?;
    b.write_all(&current)
        .and_then(|_| b.sync_all())
        .map_err(|_| "dictionary_unavailable")?;
    f.seek(SeekFrom::Start(0))
        .and_then(|_| f.write_all(new))
        .and_then(|_| f.set_len(new.len() as u64))
        .and_then(|_| f.sync_all())
        .map_err(|_| "dictionary_unavailable")?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_io_errors_distinguish_missing_permission_and_full() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            dictionary_io_error(Error::from(ErrorKind::NotFound)),
            "dictionary_missing"
        );
        assert_eq!(
            dictionary_io_error(Error::from(ErrorKind::PermissionDenied)),
            "dictionary_permission_denied"
        );
        assert_eq!(
            dictionary_io_error(Error::from(ErrorKind::StorageFull)),
            "dictionary_disk_full"
        );
        assert_eq!(
            dictionary_io_error(Error::from(ErrorKind::Other)),
            "dictionary_unavailable"
        );
    }
    #[test]
    fn category_is_optional_for_existing_clients_and_defaults_to_user() {
        let temp = tempfile::tempdir().unwrap();
        initialize(temp.path()).unwrap();
        let revision = user_revision(temp.path()).unwrap();
        let input: TermEdit = serde_json::from_value(serde_json::json!({
            "id": null, "addVariants": ["가상 용어"], "expectedRevision": revision
        }))
        .unwrap();
        let id = edit_term(temp.path(), input).unwrap();
        let listed = list(temp.path()).unwrap();
        let item = listed.terms.iter().find(|term| term.term.id == id).unwrap();
        assert_eq!(item.term.category, "사용자");
        assert_eq!(item.user_kind, Some("added"));
        assert!(item.can_delete);
    }
    #[test]
    fn search_only_matches_hala_asana_without_showing_it_as_display_variant() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let list = list(dir.path()).unwrap();
        let matches: Vec<_> = list
            .terms
            .iter()
            .filter(|t| t.search_only.iter().any(|v| v.text == "할 아사나"))
            .collect();
        assert!(matches.iter().any(|t| t.term.id == "yoga-036"));
        let hala = matches.iter().find(|t| t.term.id == "yoga-036").unwrap();
        assert!(!hala.term.variants.iter().any(|v| v.text == "할 아사나"));
        assert!(hala.search_only.iter().all(|v| v.recognition_only));
    }
    #[test]
    fn edits_user_files_only_preserve_unknown_fields_and_reject_stale_revision() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let root = dir.path();
        let base_before = fs::read(root.join(DIRECTORY).join(BASE)).unwrap();
        let initial = list(root).unwrap();
        let id = edit_term(
            root,
            TermEdit {
                id: Some("yoga-036".into()),
                add_variants: vec!["할라 예시".into()],
                remove_variants: vec![],
                enabled: Some(false),
                meaning: None,
                category: None,
                expected_revision: initial.user_revision.clone(),
            },
        )
        .unwrap();
        assert_eq!(id, "yoga-036");
        assert_eq!(
            fs::read(root.join(DIRECTORY).join(BASE)).unwrap(),
            base_before
        );
        let after = list(root).unwrap();
        let term = after.terms.iter().find(|t| t.term.id == id).unwrap();
        assert_eq!(term.origin, "user");
        assert_eq!(term.user_kind, Some("modified"));
        assert!(!term.can_delete);
        assert!(!term.term.enabled);
        assert!(term.term.variants.iter().any(|v| v.text == "할라 예시"));
        assert!(term.search_only.iter().any(|v| v.text == "할 아사나"));
        let stale = edit_term(
            root,
            TermEdit {
                id: None,
                add_variants: vec!["새 표현".into()],
                remove_variants: vec![],
                enabled: None,
                meaning: None,
                category: None,
                expected_revision: initial.user_revision,
            },
        );
        assert!(matches!(stale,Err(e) if e=="dictionary_changed"));
        let custom = edit_term(
            root,
            TermEdit {
                id: None,
                add_variants: vec!["가상 용어".into()],
                remove_variants: vec![],
                enabled: None,
                meaning: Some("가상 뜻".into()),
                category: Some("호흡".into()),
                expected_revision: after.user_revision.clone(),
            },
        )
        .unwrap();
        let updated = list(root).unwrap();
        assert!(updated.terms.iter().any(|t| t.term.id == custom
            && t.term.enabled
            && t.term.category == "호흡"
            && t.user_kind == Some("added")
            && t.can_delete
            && t.term.variants.iter().any(|v| v.text == "가상 용어")));
        edit_term(
            root,
            TermEdit {
                id: Some(custom.clone()),
                add_variants: vec!["새 가상 표기".into()],
                remove_variants: vec!["가상 용어".into()],
                enabled: Some(false),
                meaning: None,
                category: None,
                expected_revision: updated.user_revision,
            },
        )
        .unwrap();
        let updated = list(root).unwrap();
        let saved = updated.terms.iter().find(|t| t.term.id == custom).unwrap();
        assert!(
            !saved.term.enabled
                && saved.term.variants.iter().any(|v| v.text == "새 가상 표기")
                && !saved.term.variants.iter().any(|v| v.text == "가상 용어")
        );
        assert_eq!(
            delete_term(root, "yoga-036", &updated.user_revision).unwrap(),
            "base_term"
        );
        assert!(
            matches!(delete_term(root, &custom, &after.user_revision), Err(e) if e == "dictionary_changed")
        );
        assert_eq!(
            delete_term(root, &custom, &updated.user_revision).unwrap(),
            "deleted"
        );
        let deleted = list(root).unwrap();
        assert!(!deleted.terms.iter().any(|t| t.term.id == custom));
        assert!(deleted
            .terms
            .iter()
            .any(|t| t.term.id == "yoga-036" && !t.term.enabled));
        assert_eq!(
            fs::read(root.join(DIRECTORY).join(BASE)).unwrap(),
            base_before
        );
        let person = edit_person(
            root,
            PersonEdit {
                id: None,
                name: "가상 이름".into(),
                aliases: vec!["가상 별명".into()],
                confirmed_by_user: false,
                expected_revision: after.people_revision,
            },
        )
        .unwrap();
        let mut current = list(root).unwrap();
        assert!(!current.people[0].confirmed);
        let id2 = edit_person(
            root,
            PersonEdit {
                id: Some(person.clone()),
                name: "가상 이름".into(),
                aliases: vec![],
                confirmed_by_user: true,
                expected_revision: current.people_revision,
            },
        )
        .unwrap();
        assert_eq!(id2, person);
        current = list(root).unwrap();
        assert!(current.people[0].confirmed);
        delete_person(root, &person, &current.people_revision).unwrap();
        assert!(list(root).unwrap().people.is_empty());
    }
    #[test]
    fn legacy_people_are_unconfirmed_and_preserved_until_edited() {
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let root = dir.path();
        fs::write(
            root.join(DIRECTORY).join(PEOPLE),
            r#"{"people":["가상 이름",{"name":"다른 가상 이름","custom":"keep"}],"extra":"keep"}"#,
        )
        .unwrap();
        let before = list(root).unwrap();
        assert_eq!(before.people.len(), 2);
        assert!(before.people.iter().all(|p| !p.confirmed));
        edit_person(
            root,
            PersonEdit {
                id: before.people[1].id.clone(),
                name: "다른 가상 이름".into(),
                aliases: vec![],
                confirmed_by_user: false,
                expected_revision: before.people_revision,
            },
        )
        .unwrap();
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(DIRECTORY).join(PEOPLE)).unwrap()).unwrap();
        assert_eq!(raw["extra"], "keep");
        assert_eq!(raw["people"][0], "가상 이름");
        assert_eq!(raw["people"][1]["custom"], "keep");
    }
    #[cfg(windows)]
    #[test]
    fn external_writer_blocks_update_without_overwriting() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        initialize(dir.path()).unwrap();
        let root = dir.path();
        let before = list(root).unwrap();
        let path = root.join(DIRECTORY).join(USER);
        let original = fs::read(&path).unwrap();
        let held = fs::OpenOptions::new()
            .write(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        let result = edit_term(
            root,
            TermEdit {
                id: None,
                add_variants: vec!["가상 표기".into()],
                remove_variants: vec![],
                enabled: None,
                meaning: None,
                category: None,
                expected_revision: before.user_revision,
            },
        );
        assert!(matches!(result,Err(e) if e=="dictionary_unavailable" || e=="dictionary_busy"));
        drop(held);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    #[cfg(windows)]
    fn known_clean_base_update_is_saved_without_user_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        crate::archive::initialize(&root, temp.path()).unwrap();
        let tool = crate::archive::storage_tool(temp.path()).unwrap().0;
        fs::write(root.join("dictionary/yoga.json"), PREVIOUS_BUNDLES[0]).unwrap();
        crate::archive::run_storage_tool(&tool, &root, &["add", "--", "dictionary/yoga.json"])
            .unwrap();
        crate::archive::run_storage_tool(
            &tool,
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "previous dictionary",
            ],
        )
        .unwrap();
        fs::write(root.join("dictionary/people.json"), "user names").unwrap();
        initialize(&root).unwrap();
        assert_eq!(
            fs::read(root.join("dictionary/yoga.json")).unwrap(),
            BASE_CONTENT.as_bytes()
        );
        assert_eq!(
            fs::read(root.join("dictionary/people.json")).unwrap(),
            b"user names"
        );
        let message = crate::archive::run_storage_tool(
            &tool,
            &root,
            &["log", "-1", "--format=%s", "--", "dictionary/yoga.json"],
        )
        .unwrap();
        assert_eq!(message.trim(), "몸소 업데이트: 기본 용어 사전 갱신");
        let saved = crate::archive::run_storage_tool(
            &tool,
            &root,
            &["show", "--format=", "--name-only", "HEAD"],
        )
        .unwrap();
        assert!(!saved.contains("dictionary/people.json"));
    }
    #[test]
    fn fresh_dictionary_has_confirmed_candidates_and_no_history() {
        let d = tempfile::tempdir().unwrap();
        initialize(d.path()).unwrap();
        assert!(defaults_unchanged(d.path()));
        let listed = list(d.path()).unwrap();
        assert!(listed.terms.len() > 137);
        assert!(listed.fragments.len() >= 20);
        let raw: serde_json::Value = serde_json::from_str(BASE_CONTENT).unwrap();
        assert!(raw["terms"]
            .as_array()
            .unwrap()
            .iter()
            .all(|term| term.get("preferred").is_none()));
        assert!(
            serde_json::to_value(list(d.path()).unwrap()).unwrap()["terms"]
                .as_array()
                .unwrap()
                .iter()
                .all(|term| term.get("preferred").is_none())
        );
        assert!(!d.path().join(".git").exists());
    }
    #[test]
    fn screen_hides_generated_variants_and_marks_shared_names() {
        let d = tempfile::tempdir().unwrap();
        initialize(d.path()).unwrap();
        let list = list(d.path()).unwrap();
        assert!(list
            .terms
            .iter()
            .flat_map(|t| &t.term.variants)
            .all(|v| !v.recognition_only
                && ((v.origin == "observed" && !v.sources.is_empty())
                    || (v.origin == "source_unverified"
                        && !v.unverified_sources.is_empty()
                        && v.requires_confirmation))));
        let shared: Vec<_> = list
            .terms
            .iter()
            .flat_map(|t| &t.term.variants)
            .filter(|v| v.text == "태아 자세")
            .collect();
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|v| v.requires_confirmation));
        let baka = list.terms.iter().find(|t| t.term.id == "yoga-053").unwrap();
        assert!(baka.term.variants.iter().all(|v| v.requires_confirmation));
        let raw: Bundle = serde_json::from_str(BASE_CONTENT).unwrap();
        let hala = raw.terms.iter().find(|t| t.id == "yoga-036").unwrap();
        assert!(hala.variants.iter().any(|v| v.text == "할 아사나"
            && v.recognition_only
            && v.origin == "rule_generated"
            && v.sources.is_empty()));
    }
    #[test]
    fn user_term_wins_and_changed_or_removed_base_is_reported() {
        let d = tempfile::tempdir().unwrap();
        initialize(d.path()).unwrap();
        let term = list(d.path()).unwrap().terms.remove(0).term;
        let mut edited = serde_json::to_value(&term).unwrap();
        edited["meaning"] = "person's meaning".into();
        // Older user files may still have this field; reading must not enforce it
        // or rewrite the person's file.
        edited["preferred"] = "old value".into();
        let mut removed = edited.clone();
        removed["id"] = "yoga-old".into();
        let value = serde_json::json!({"terms":[{"term":edited,"baseMeaning":"old base meaning"},{"term":removed,"baseMeaning":"old"}]});
        fs::write(
            d.path().join(DIRECTORY).join(USER),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let result = list(d.path()).unwrap();
        assert_eq!(result.terms[0].term.meaning, "person's meaning");
        assert!(serde_json::to_value(&result.terms[0].term)
            .unwrap()
            .get("preferred")
            .is_none());
        assert!(fs::read_to_string(d.path().join(DIRECTORY).join(USER))
            .unwrap()
            .contains("old value"));
        assert_eq!(result.terms[0].conflict, Some("base_changed"));
        assert_eq!(result.terms.last().unwrap().conflict, Some("base_removed"));
    }
    #[test]
    #[cfg(windows)]
    fn previous_bundle_is_upgraded_without_rewriting_user_files() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join(".git")).unwrap();
        initialize(d.path()).unwrap();
        let base = d.path().join(DIRECTORY).join(BASE);
        let user = d.path().join(DIRECTORY).join(USER);
        let people = d.path().join(DIRECTORY).join(PEOPLE);
        fs::write(&user, b"user terms").unwrap();
        fs::write(&people, b"user names").unwrap();
        for old in PREVIOUS_BUNDLES {
            fs::write(&base, old).unwrap();
            initialize(d.path()).unwrap();
            assert_eq!(fs::read(&base).unwrap(), BASE_CONTENT.as_bytes());
            assert_eq!(fs::read(&user).unwrap(), b"user terms");
            assert_eq!(fs::read(&people).unwrap(), b"user names");
        }
        fs::write(&base, b"user-edited base").unwrap();
        initialize(d.path()).unwrap();
        assert_eq!(fs::read(&base).unwrap(), b"user-edited base");
    }
    #[test]
    fn fragment_data_keeps_evidence_and_labels_generated_forms() {
        let b: Bundle = serde_json::from_str(BASE_CONTENT).unwrap();
        assert!(b.fragments.iter().any(|f| f.id == "fragment-parivrtta"
            && f.variants
                .iter()
                .any(|v| v.text == "파리브리타" && !v.sources.is_empty())));
        assert!(b.fragments.iter().any(|f| f.id == "fragment-trikona"));
        assert!(b.fragments.iter().any(|f| f.id == "fragment-asana"));
        assert!(b
            .terms
            .iter()
            .flat_map(|t| &t.variants)
            .any(|v| v.origin == "rule_generated"
                && v.recognition_only
                && v.generated_from.contains(&"fragment-parivrtta".to_string())));
        let mut owners: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
        for t in &b.terms {
            for v in &t.variants {
                owners.entry(&v.text).or_default().insert(&t.id);
            }
        }
        for t in &b.terms {
            for v in &t.variants {
                if owners.get(v.text.as_str()).unwrap().len() > 1 {
                    assert!(v.requires_confirmation);
                }
            }
        }
        for t in &b.terms {
            assert!(!t.verified_sources.is_empty(), "{}", t.id);
            for v in &t.variants {
                if v.origin == "observed" {
                    assert!(!v.sources.is_empty(), "{}", t.id);
                }
                if v.origin == "rule_generated" {
                    assert!(v.sources.is_empty() && v.recognition_only, "{}", t.id);
                }
            }
        }
    }
    #[test]
    #[cfg(windows)]
    fn update_preserves_person_files_and_rejects_edited_base() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir(d.path().join(".git")).unwrap();
        initialize(d.path()).unwrap();
        let user = d.path().join(DIRECTORY).join(USER);
        let original = list(d.path()).unwrap().terms.remove(0).term;
        let mut edited = serde_json::to_value(&original).unwrap();
        edited["meaning"] = "person's data".into();
        let user_bytes = serde_json::to_vec(
            &serde_json::json!({"terms":[{"term":edited,"baseMeaning":original.meaning}]}),
        )
        .unwrap();
        fs::write(&user, &user_bytes).unwrap();
        let people = d.path().join(DIRECTORY).join(PEOPLE);
        fs::write(&people, "person's names").unwrap();
        let mut next: serde_json::Value = serde_json::from_str(BASE_CONTENT).unwrap();
        next["terms"][0]["meaning"] = "changed meaning".into();
        let next = serde_json::to_vec(&next).unwrap();
        let old_bundle: Bundle = serde_json::from_str(BASE_CONTENT).unwrap();
        let new_bundle: Bundle = serde_json::from_slice(&next).unwrap();
        let conflicts = base_conflicts(&old_bundle, &new_bundle);
        assert_eq!(conflicts[0].reason, "base_changed");
        let mut removed_bundle: Bundle = serde_json::from_slice(&next).unwrap();
        removed_bundle.terms.remove(0);
        assert_eq!(
            base_conflicts(&old_bundle, &removed_bundle)[0].reason,
            "base_removed"
        );
        assert!(replace_known_base(d.path(), BASE_CONTENT.as_bytes(), &next).unwrap());
        assert_eq!(fs::read(&user).unwrap(), user_bytes);
        assert_eq!(fs::read(&people).unwrap(), b"person's names");
        let listed = list(d.path()).unwrap();
        assert_eq!(listed.terms[0].term.meaning, "person's data");
        assert_eq!(listed.terms[0].conflict, Some("base_changed"));
        assert!(!replace_known_base(d.path(), BASE_CONTENT.as_bytes(), b"other").unwrap());
        assert_eq!(fs::read(d.path().join(DIRECTORY).join(BASE)).unwrap(), next);
    }
}
