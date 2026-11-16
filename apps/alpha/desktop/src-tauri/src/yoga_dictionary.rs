use serde::{Deserialize, Serialize};
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

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Term {
    id: String,
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
    source_file: String,
    conflict: Option<&'static str>,
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

fn read_safe(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    let path = dir.join(name);
    let meta = fs::symlink_metadata(&path).map_err(|_| "dictionary_unavailable")?;
    if !meta.is_file() || crate::archive_location::is_link_or_reparse(&meta) {
        return Err("dictionary_unsafe".into());
    }
    fs::read(path).map_err(|_| "dictionary_unavailable".into())
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
    for previous in PREVIOUS_BUNDLES {
        let old: Bundle = serde_json::from_str(previous).map_err(|_| "dictionary_invalid")?;
        conflicts.extend(base_conflicts(&old, &bundle));
    }
    let user: UserTerms =
        serde_json::from_slice(&read_safe(&dir, USER)?).map_err(|_| "dictionary_invalid")?;
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
                source_file: format!("{DIRECTORY}/{USER}"),
                conflict,
            });
        } else {
            terms.push(ListedTerm {
                term: t,
                origin: "default",
                source_file: format!("{DIRECTORY}/{BASE}"),
                conflict: None,
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
        terms.push(ListedTerm {
            term: entry.term,
            origin: "user",
            source_file: format!("{DIRECTORY}/{USER}"),
            conflict,
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
        item.term.variants.retain(|v| {
            ["observed", "source_unverified"].contains(&v.origin.as_str()) && !v.recognition_only
        });
    }
    Ok(DictionaryList {
        bundle_version: bundle.bundle_version,
        review_status: bundle.review_status,
        base_status,
        base_conflicts: conflicts,
        terms,
        fragments: bundle.fragments,
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
