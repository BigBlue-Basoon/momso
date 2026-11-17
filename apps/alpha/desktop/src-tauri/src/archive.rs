use serde::Serialize;
use std::io::Write;
const ARCHIVE_RULES: &str = include_str!("archive_rules.md");
const RULE_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "GEMINI.md"];
const PREVIOUS_RULE_HASHES: [&str; 10] = [
    "2f1f610ebd2c56e38dd119b5946167924d6ff9692e274553732d1db1a6020903",
    "7b000c37a5ff805250fc3b7b01f666ac55f87d06ece27e7eb4b81d40987467da",
    "b29284f807489bfa9f76d2639f3355d6776ff6a7023713007b37a5afa9624c24",
    "a8302846395936f0c2282ebe581b1133ca2745f5285a05c2d296f1220ba974dc",
    "5c14c28814896e6597ccea941dd479fd34ad8239d37a86c0480838eb92a6e458",
    "cb50058d83310c64913f37bda14d0c8f71416422e0a2f122b1ba2ad0ab34bd79",
    "566db8241fc4f34189c827e1a5910317ea816abb19579e0de036475dad1ea3d1",
    "a5f3435d11897bbf63cb7c1fe386d334a63aa02c8b1f2eb3ba68b4a7d34882c0",
    "ef1d49879af6a2ccf3aa0d3151c3a89fc8a6801ac6c36dac50b45a0952b2d7a1",
    "7e4067c7f95c8d29b362adc6a74904577f0eb9c45d084f1566851c6f68401a2f",
];
fn normalized_rule_text(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes)
        .ok()
        .map(|s| s.replace("\r\n", "\n"))
}

// Only resolve a known prior bundle against the current bundle when the
// user-profile suffix is identical on both sides.
pub(crate) fn select_managed_rule(local: &str, remote: &str) -> Option<bool> {
    use sha2::{Digest, Sha256};
    const PROFILE: &str = "\n## 사용자에 대해\n\n사용자 정보가 몸소 보관함 규칙과 어긋나면 몸소 보관함 규칙을 먼저 따른다.\n";
    let local = normalized_rule_text(local.as_bytes())?;
    let remote = normalized_rule_text(remote.as_bytes())?;
    let current = normalized_rule_text(ARCHIVE_RULES.as_bytes())?;
    fn split(text: &str) -> (&str, &str) {
        match text.find(PROFILE) {
            Some(offset) => (&text[..offset], &text[offset..]),
            None => (text, ""),
        }
    }
    let (local_base, local_profile) = split(&local);
    let (remote_base, remote_profile) = split(&remote);
    if local_profile != remote_profile {
        return None;
    }
    let known = |text: &str| {
        PREVIOUS_RULE_HASHES.contains(&format!("{:x}", Sha256::digest(text.as_bytes())).as_str())
    };
    match (local_base == current, remote_base == current) {
        (true, true) => Some(true),
        (true, false) if known(remote_base) => Some(false),
        (false, true) if known(local_base) => Some(true),
        _ => None,
    }
}
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveStatus {
    path: String,
    zet_count: usize,
    receipt_count: usize,
    has_unsaved_changes: bool,
    uses_bundled_git: bool,
    rules_modified: bool,
}

pub(crate) fn storage_tool(_resources: &Path) -> Result<(PathBuf, bool), String> {
    crate::git_tool::resolve()
        .map(|(path, _)| (path, false))
        .ok_or_else(|| "저장 도구를 찾지 못했습니다".into())
}

pub(crate) fn run_storage_tool(
    executable: &Path,
    root: &Path,
    args: &[&str],
) -> Result<String, String> {
    let output = Command::new(executable)
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "저장 도구를 실행하지 못했습니다".to_string())?;
    if !output.status.success() {
        return Err("몸소 폴더 저장 작업을 완료하지 못했습니다".into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn commit_paths(
    executable: &Path,
    root: &Path,
    message: &str,
    paths: &[String],
) -> Result<(), String> {
    let mut child = Command::new(executable)
        .args([
            "--literal-pathspecs",
            "-c",
            "user.name=momso",
            "-c",
            "user.email=archive@momso.local",
            "commit",
            "--only",
            "-m",
            message,
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ])
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "저장 도구를 실행하지 못했습니다".to_string())?;
    let result = (|| -> std::io::Result<()> {
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("missing stdin"))?;
        for path in paths {
            input.write_all(path.as_bytes())?;
            input.write_all(&[0])?;
        }
        Ok(())
    })();
    let output = child
        .wait_with_output()
        .map_err(|_| "저장 도구를 실행하지 못했습니다".to_string())?;
    if result.is_err() || !output.status.success() {
        return Err("몸소 폴더 저장 작업을 완료하지 못했습니다".into());
    }
    Ok(())
}

pub(crate) fn initialize(root: &Path, resources: &Path) -> Result<(), String> {
    let (executable, _) = storage_tool(resources)?;
    for name in ["zettels", "objects", "receipts"] {
        fs::create_dir_all(root.join(name))
            .map_err(|_| "몸소 폴더를 만들지 못했습니다".to_string())?;
    }
    let profile = profile_get(root)?;
    let expected_rules = profile_rules(&profile);
    let _rule_guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "몸소 폴더 설정을 확인하지 못했습니다".to_string())?;
    let has_history = root.join(".git").exists()
        && run_storage_tool(&executable, root, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let profile_clean = !has_history
        || run_storage_tool(
            &executable,
            root,
            &["status", "--porcelain", "--", "profile.md"],
        )
        .is_ok_and(|status| status.trim().is_empty());
    let mut upgraded: Vec<(&str, Vec<u8>)> = Vec::new();
    let legacy_rules = expected_rules.replacen(
        if ARCHIVE_RULES.contains("\r\n") {
            "- `skills/` 와 `.agents/skills/`, `.claude/skills/` 는 몸소가 관리한다. AI는 사람이 시켰을 때만 스킬을 고친다.\r\n"
        } else {
            "- `skills/` 와 `.agents/skills/`, `.claude/skills/` 는 몸소가 관리한다. AI는 사람이 시켰을 때만 스킬을 고친다.\n"
        }, "", 1);
    for name in RULE_FILES {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join(name))
        {
            Ok(mut file) => file
                .write_all(expected_rules.as_bytes())
                .map_err(|_| "규칙 파일을 만들지 못했습니다".to_string())?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                use sha2::{Digest, Sha256};
                let path = root.join(name);
                let contents =
                    fs::read(&path).map_err(|_| "규칙 파일을 읽지 못했습니다".to_string())?;
                let normalized_base = normalized_rule_text(ARCHIVE_RULES.as_bytes())
                    .expect("embedded rules are UTF-8");
                let expected = profile_rules(&profile);
                let normalized_expected =
                    normalized_rule_text(expected.as_bytes()).expect("profile is UTF-8");
                let suffix = &normalized_expected[normalized_base.len()..];
                let normalized_contents = normalized_rule_text(&contents);
                let prior_base = normalized_contents
                    .as_deref()
                    .and_then(|text| text.strip_suffix(suffix));
                let is_previous = prior_base.is_some_and(|base| {
                    let hash = format!("{:x}", Sha256::digest(base.as_bytes()));
                    PREVIOUS_RULE_HASHES.contains(&hash.as_str()) || base == normalized_base
                }) || normalized_contents
                    == normalized_rule_text(legacy_rules.as_bytes());
                if is_previous {
                    if normalized_contents == normalized_rule_text(expected.as_bytes()) {
                        continue;
                    }
                    let clean = has_history
                        && profile_clean
                        && run_storage_tool(
                            &executable,
                            root,
                            &["status", "--porcelain", "--", name],
                        )
                        .is_ok_and(|status| status.trim().is_empty())
                        && run_storage_tool(&executable, root, &["show", &format!("HEAD:{name}")])
                            .is_ok_and(|saved| {
                                normalized_rule_text(saved.as_bytes()) == normalized_contents
                            });
                    if has_history && !clean {
                        continue;
                    }
                    fs::write(path, expected_rules.as_bytes())
                        .map_err(|_| "규칙 파일을 갱신하지 못했습니다".to_string())?;
                    if clean {
                        upgraded.push((name, contents));
                    }
                }
            }
            Err(_) => return Err("규칙 파일을 만들지 못했습니다".into()),
        }
    }
    let ignore = root.join(".gitignore");
    if ignore.exists() {
        let current =
            fs::read(&ignore).map_err(|_| "몸소 폴더 설정을 읽지 못했습니다".to_string())?;
        if current != b"objects/\n" && current != b"objects/\r\n" {
            return Err("몸소 폴더의 원자료 제외 설정을 확인해 주세요".into());
        }
    } else {
        fs::write(&ignore, b"objects/\n")
            .map_err(|_| "몸소 폴더 설정을 만들지 못했습니다".to_string())?;
    }
    if !root.join(".git").exists() {
        run_storage_tool(&executable, root, &["init", "--initial-branch=main"])?;
    }
    if !upgraded.is_empty() {
        if upgraded.iter().any(|(name, _)| {
            fs::read(root.join(name)).ok().as_deref() != Some(expected_rules.as_bytes())
                || run_storage_tool(
                    &executable,
                    root,
                    &["diff", "--cached", "--quiet", "--", name],
                )
                .is_err()
        }) {
            return Err("규칙 파일이 그사이 바뀌었습니다".to_string());
        }
        let mut args = vec![
            "-c",
            "user.name=momso",
            "-c",
            "user.email=archive@momso.local",
            "commit",
            "--only",
            "-m",
            "몸소 업데이트: 기본 규칙 갱신",
            "--",
        ];
        args.extend(upgraded.iter().map(|(name, _)| *name));
        if let Err(error) = run_storage_tool(&executable, root, &args) {
            for (name, before) in &upgraded {
                fs::write(root.join(name), before)
                    .map_err(|_| "규칙 파일 복구를 완료하지 못했습니다".to_string())?;
            }
            return Err(error);
        }
    }
    crate::yoga_dictionary::initialize(root)?;
    drop(_rule_guard);
    crate::skills::install_bundled(root, resources).map_err(str::to_owned)?;
    Ok(())
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UserProfile {
    name: String,
    about: String,
    style: String,
    updated_at: Option<String>,
}

pub(crate) fn profile_get(root: &Path) -> Result<UserProfile, String> {
    let path = root.join("profile.md");
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserProfile::default())
        }
        Err(_) => return Err("사용자 정보를 읽지 못했습니다".into()),
    };
    let yaml = text
        .strip_prefix("---\n")
        .and_then(|s| s.strip_suffix("---\n"))
        .ok_or_else(|| "사용자 정보 파일을 읽지 못했습니다".to_string())?;
    serde_yaml::from_str(yaml).map_err(|_| "사용자 정보 파일을 읽지 못했습니다".into())
}

fn profile_rules(profile: &UserProfile) -> String {
    let mut text =
        normalized_rule_text(ARCHIVE_RULES.as_bytes()).expect("embedded rules are UTF-8");
    if !profile.name.is_empty() || !profile.about.is_empty() || !profile.style.is_empty() {
        text.push_str("\n## 사용자에 대해\n\n사용자 정보가 몸소 보관함 규칙과 어긋나면 몸소 보관함 규칙을 먼저 따른다.\n");
        for (label, value) in [
            ("이름", &profile.name),
            ("나에 대해", &profile.about),
            ("답하는 말투", &profile.style),
        ] {
            if !value.is_empty() {
                text.push_str(&format!("\n### {label}\n\n{value}\n"));
            }
        }
    }
    text
}

pub(crate) fn profile_set(
    root: &Path,
    resources: &Path,
    name: String,
    about: String,
    style: String,
) -> Result<UserProfile, String> {
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "사용자 정보를 저장하지 못했습니다".to_string())?;
    if name.contains(['\n', '\r']) {
        return Err("이름은 한 줄로 적어 주세요".into());
    }
    if [&name, &about, &style]
        .iter()
        .any(|s| s.chars().count() > 4000)
    {
        return Err("사용자 정보는 각 항목을 4,000자 이내로 적어 주세요".into());
    }
    let old = profile_get(root)?;
    let old_rules = profile_rules(&old);
    let prior_profile = match fs::read(root.join("profile.md")) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err("사용자 정보를 읽지 못했습니다".into()),
    };
    let mut prior_rules = Vec::new();
    for file in RULE_FILES {
        let prior = match fs::read(root.join(file)) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("규칙 파일을 읽지 못했습니다".into()),
        };
        if prior
            .as_deref()
            .is_some_and(|v| normalized_rule_text(v) != normalized_rule_text(old_rules.as_bytes()))
        {
            return Err("사용자가 고친 규칙 파일을 확인해 주세요".into());
        }
        prior_rules.push((file, prior));
    }
    let (tool, _) = storage_tool(resources)?;
    let files = ["profile.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md"];
    let mut indices = Vec::new();
    for file in files {
        let index = run_storage_tool(&tool, root, &["ls-files", "--stage", "--", file])?;
        let staged = run_storage_tool(
            &tool,
            root,
            &["diff", "--cached", "--name-only", "--", file],
        )?;
        if !staged.trim().is_empty() || index.lines().count() > 1 {
            return Err("사용자 정보의 저장 상태를 확인해 주세요".into());
        }
        indices.push((file, index));
    }
    let profile = UserProfile {
        name,
        about,
        style,
        updated_at: Some(chrono::Local::now().to_rfc3339()),
    };
    let yaml = serde_yaml::to_string(&profile)
        .map_err(|_| "사용자 정보를 만들지 못했습니다".to_string())?;
    let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
    let contents = format!("---\n{yaml}---\n");
    let rules = profile_rules(&profile);
    let result = (|| -> Result<(), String> {
        fs::write(root.join("profile.md"), contents.as_bytes())
            .map_err(|_| "사용자 정보를 쓰지 못했습니다".to_string())?;
        for file in RULE_FILES {
            fs::write(root.join(file), rules.as_bytes())
                .map_err(|_| "규칙 파일을 갱신하지 못했습니다".to_string())?;
        }
        run_storage_tool(
            &tool,
            root,
            &[
                "add",
                "--",
                "profile.md",
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
            ],
        )?;
        run_storage_tool(
            &tool,
            root,
            &[
                "-c",
                "user.name=momso",
                "-c",
                "user.email=archive@momso.local",
                "commit",
                "--only",
                "-m",
                "저장: 사용자 정보",
                "--",
                "profile.md",
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
            ],
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let restore = |path: &Path, value: &Option<Vec<u8>>| -> bool {
            match value {
                Some(bytes) => fs::write(path, bytes).is_ok(),
                None => match fs::remove_file(path) {
                    Ok(()) => true,
                    Err(e) => e.kind() == std::io::ErrorKind::NotFound,
                },
            }
        };
        let mut ok = restore(&root.join("profile.md"), &prior_profile);
        for (file, previous) in &prior_rules {
            ok &= restore(&root.join(file), previous);
        }
        for (file, index) in indices {
            if let Some(line) = index.lines().next() {
                let parts: Vec<_> = line.split_whitespace().collect();
                ok &= parts.len() >= 3
                    && run_storage_tool(
                        &tool,
                        root,
                        &[
                            "update-index",
                            "--add",
                            "--cacheinfo",
                            parts[0],
                            parts[1],
                            file,
                        ],
                    )
                    .is_ok();
            } else {
                ok &=
                    run_storage_tool(&tool, root, &["update-index", "--force-remove", "--", file])
                        .is_ok();
            }
        }
        return Err(if ok {
            format!("사용자 정보 저장 실패: {error}; 이전 상태로 돌렸습니다")
        } else {
            format!("사용자 정보 저장 실패: {error}; 이전 상태를 모두 되돌리지 못했습니다")
        });
    }
    Ok(profile)
}

fn count_files(root: &Path, folder: &str) -> Result<usize, String> {
    let entries = fs::read_dir(root.join(folder))
        .map_err(|_| "몸소 폴더 목록을 읽지 못했습니다".to_string())?;
    let mut count = 0;
    for entry in entries {
        let entry = entry.map_err(|_| "몸소 폴더 목록을 읽지 못했습니다".to_string())?;
        if entry
            .file_type()
            .map_err(|_| "몸소 폴더 목록을 읽지 못했습니다".to_string())?
            .is_file()
            && entry.path().extension().is_some_and(|ext| ext == "md")
        {
            count += 1;
        }
    }
    Ok(count)
}

pub(crate) fn allocate_file(
    folder: &Path,
    prefix: &str,
) -> Result<(String, String, fs::File), String> {
    allocate_with_clock(
        folder,
        prefix,
        || {
            let now = chrono::Local::now();
            (
                now.format("%Y%m%d_%H%M%S").to_string(),
                now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
            )
        },
        || std::thread::sleep(std::time::Duration::from_millis(100)),
    )
}

fn allocate_with_clock(
    folder: &Path,
    prefix: &str,
    mut clock: impl FnMut() -> (String, String),
    mut wait: impl FnMut(),
) -> Result<(String, String, fs::File), String> {
    loop {
        let (suffix, timestamp) = clock();
        let id = format!("{prefix}_{suffix}");
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(folder.join(format!("{id}.md")))
        {
            Ok(file) => return Ok((id, timestamp, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => wait(),
            Err(_) => return Err("파일을 만들지 못했습니다".into()),
        }
    }
}

pub(crate) fn status(root: &Path, resources: &Path) -> Result<ArchiveStatus, String> {
    let (executable, bundled) = storage_tool(resources)?;
    let output = run_storage_tool(
        &executable,
        root,
        &[
            "status",
            "--porcelain",
            "--",
            "zettels/",
            "receipts/",
            "AGENTS.md",
            "CLAUDE.md",
            "GEMINI.md",
        ],
    )?;
    let expected_rules = profile_rules(&profile_get(root)?);
    Ok(ArchiveStatus {
        path: root.to_string_lossy().into_owned(),
        zet_count: count_files(root, "zettels")?,
        receipt_count: count_files(root, "receipts")?,
        has_unsaved_changes: !output.trim().is_empty(),
        uses_bundled_git: bundled,
        rules_modified: RULE_FILES.iter().any(|name| {
            fs::read(root.join(name)).is_ok_and(|contents| {
                normalized_rule_text(&contents) != normalized_rule_text(expected_rules.as_bytes())
            })
        }),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReindexResult {
    zets: usize,
    receipts: usize,
}

pub(crate) fn reindex(root: &Path, database: &Path) -> Result<ReindexResult, String> {
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
    let mut zets = Vec::new();
    let mut receipts = Vec::new();
    for (folder, is_zet) in [("zettels", true), ("receipts", false)] {
        for entry in fs::read_dir(root.join(folder))
            .map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?
        {
            let entry = entry.map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?;
            if !entry
                .file_type()
                .map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?
                .is_file()
                || entry.path().extension().is_none_or(|x| x != "md")
            {
                continue;
            }
            let text = fs::read_to_string(entry.path())
                .map_err(|_| "몸소 폴더 파일을 읽지 못했습니다".to_string())?;
            let filename = entry.file_name().to_string_lossy().into_owned();
            if is_zet {
                let (head, _) = crate::archive_zet::parse(&text)?;
                if filename != format!("{}.md", head.id) {
                    return Err("글 번호와 파일 이름이 다릅니다".into());
                }
                zets.push(head);
            } else {
                let (head, _) = crate::archive_receipt::parse(&text)?;
                if filename != format!("{}.md", head.id) {
                    return Err("작업 기록 번호와 파일 이름이 다릅니다".into());
                }
                receipts.push(head);
            }
        }
    }
    let mut conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "색인을 열지 못했습니다".to_string())?;
    let tx = conn
        .transaction()
        .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
    tx.execute_batch("DELETE FROM archive_zets_assets; DELETE FROM archive_zets_edges; DELETE FROM archive_zets; DELETE FROM archive_receipts_read; DELETE FROM archive_receipts_wrote; DELETE FROM archive_receipts;").map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
    #[cfg(test)]
    if std::env::var_os("MOMSO_C28_CRASH_DURING_INDEX").is_some() {
        std::process::exit(73);
    }
    for h in &zets {
        tx.execute("INSERT INTO archive_zets(id,title,summary,kind,status,created_at,updated_at,file_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", rusqlite::params![h.id,h.title,h.summary,h.kind,h.status,h.created_at,h.updated_at,format!("zettels/{}.md",h.id)]).map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
        for asset in &h.assets {
            tx.execute(
                "INSERT OR IGNORE INTO archive_zets_assets(zet_id,asset_id) VALUES (?1,?2)",
                rusqlite::params![h.id, asset],
            )
            .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
        }
        for edge in &h.edges {
            tx.execute(
                "INSERT OR IGNORE INTO archive_zets_edges(zet_id,edge_type,target_id) VALUES (?1,?2,?3)",
                rusqlite::params![h.id, edge.kind, edge.target],
            )
            .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
        }
    }
    for h in &receipts {
        tx.execute(
            "INSERT INTO archive_receipts(id,created_at,tool,model) VALUES (?1,?2,?3,?4)",
            rusqlite::params![h.id, h.created_at, h.tool, h.model],
        )
        .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
        for id in &h.read {
            tx.execute(
                "INSERT OR IGNORE INTO archive_receipts_read(receipt_id,reference_id) VALUES (?1,?2)",
                rusqlite::params![h.id, id],
            )
            .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
        }
        for id in &h.wrote {
            tx.execute(
                "INSERT OR IGNORE INTO archive_receipts_wrote(receipt_id,reference_id) VALUES (?1,?2)",
                rusqlite::params![h.id, id],
            )
            .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
        }
    }
    tx.commit()
        .map_err(|_| "색인을 다시 만들지 못했습니다".to_string())?;
    Ok(ReindexResult {
        zets: zets.len(),
        receipts: receipts.len(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryEntry {
    revision: String,
    time: String,
    title: Option<String>,
    other_zets: usize,
}

fn revision_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

pub(crate) fn history(
    root: &Path,
    resources: &Path,
    id: &str,
) -> Result<Vec<HistoryEntry>, String> {
    if !crate::archive_zet::valid_zet_id(id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    let (tool, _) = storage_tool(resources)?;
    if run_storage_tool(&tool, root, &["rev-parse", "--verify", "HEAD"]).is_err() {
        return Ok(Vec::new());
    }
    let relative = format!("zettels/{id}.md");
    let text = run_storage_tool(
        &tool,
        root,
        &[
            "-c",
            "i18n.logOutputEncoding=UTF-8",
            "log",
            "--full-history",
            "--format=%H%x1f%cI%x1f%s",
            "--",
            &relative,
        ],
    )?;
    text.lines()
        .map(|line| {
            let mut parts = line.splitn(3, '\u{1f}');
            let revision = parts.next().unwrap_or_default().to_string();
            let time = parts.next().unwrap_or_default().to_string();
            let _ = parts.next();
            if !revision_id(&revision) || time.is_empty() {
                return Err("이력을 읽지 못했습니다".into());
            }
            let text = read_version(root, resources, id, &revision)?;
            let title = crate::archive_zet::parse(&text)
                .ok()
                .map(|(head, _)| head.title);
            let changed = run_storage_tool(
                &tool,
                root,
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    &revision,
                    "--",
                    "zettels/",
                ],
            )?;
            let count = changed.lines().filter(|line| line.ends_with(".md")).count();
            Ok(HistoryEntry {
                revision,
                time,
                title,
                other_zets: count.saturating_sub(1),
            })
        })
        .collect()
}

pub(crate) fn read_version(
    root: &Path,
    resources: &Path,
    id: &str,
    revision: &str,
) -> Result<String, String> {
    if !crate::archive_zet::valid_zet_id(id) || !revision_id(revision) {
        return Err("글 번호 또는 이력이 올바르지 않습니다".into());
    }
    let (tool, _) = storage_tool(resources)?;
    let object = format!("{revision}:zettels/{id}.md");
    run_storage_tool(&tool, root, &["show", &object])
}

#[derive(Serialize)]
pub(crate) struct ArchiveIssue {
    file: String,
    message: String,
}

struct IdTimeContext {
    tool: PathBuf,
    head: Option<String>,
    additions: Option<std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>>,
}

fn linear_additions(
    tool: &Path,
    root: &Path,
    head: &str,
) -> Option<std::collections::HashMap<String, chrono::DateTime<chrono::Utc>>> {
    let parents = run_storage_tool(tool, root, &["rev-list", "--parents", head]).ok()?;
    if parents
        .lines()
        .any(|line| line.split_whitespace().count() > 2)
    {
        return None;
    }
    let renames = run_storage_tool(
        tool,
        root,
        &[
            "-c",
            "diff.renameLimit=0",
            "log",
            "--format=",
            "--name-status",
            "--diff-filter=R",
            "-M",
            head,
        ],
    )
    .ok()?;
    if !renames.trim().is_empty() {
        return None;
    }
    let history = run_storage_tool(
        tool,
        root,
        &[
            "log",
            "--reverse",
            "--format=MOMSO_TIME:%cI",
            "--name-only",
            "--diff-filter=A",
            "--no-renames",
            head,
            "--",
            "zettels/",
            "receipts/",
        ],
    )
    .ok()?;
    let mut additions = std::collections::HashMap::new();
    let mut time = None;
    for line in history
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix("MOMSO_TIME:") {
            time = Some(chrono::DateTime::parse_from_rfc3339(value).ok()?.to_utc());
        } else if line.starts_with("zettels/") || line.starts_with("receipts/") {
            additions.entry(line.to_owned()).or_insert(time?);
        } else {
            return None;
        }
    }
    Some(additions)
}

fn id_time_context(root: &Path, resources: &Path) -> Result<IdTimeContext, String> {
    let (tool, _) = storage_tool(resources)?;
    let head = run_storage_tool(&tool, root, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .map(|value| value.trim().to_owned());
    let additions = head
        .as_deref()
        .and_then(|head| linear_additions(&tool, root, head));
    Ok(IdTimeContext {
        tool,
        head,
        additions,
    })
}

fn id_time_mismatch_with_context(
    root: &Path,
    context: &IdTimeContext,
    relative: &str,
    id: &str,
) -> Result<bool, String> {
    use chrono::TimeZone;
    let number_time = chrono::NaiveDateTime::parse_from_str(&id[4..], "%Y%m%d_%H%M%S")
        .map_err(|_| "번호의 시각을 확인하지 못했습니다".to_string())?;
    let number_time = chrono::Local
        .from_local_datetime(&number_time)
        .earliest()
        .ok_or_else(|| "번호의 시각을 확인하지 못했습니다".to_string())?;
    let cached = context
        .additions
        .as_ref()
        .and_then(|items| items.get(relative))
        .copied();
    let first = if cached.is_some() {
        cached
    } else if let Some(head) = &context.head {
        let history = run_storage_tool(
            &context.tool,
            root,
            &[
                "log",
                "--follow",
                "--diff-filter=A",
                "--reverse",
                "--format=%cI",
                head,
                "--",
                relative,
            ],
        )?;
        history
            .lines()
            .next()
            .map(|line| {
                chrono::DateTime::parse_from_rfc3339(line)
                    .map(|time| time.to_utc())
                    .map_err(|_| "저장 시각을 확인하지 못했습니다".to_string())
            })
            .transpose()?
    } else {
        None
    };
    let actual = match first {
        Some(time) => time,
        None => fs::metadata(root.join(relative))
            .and_then(|m| m.created())
            .map(chrono::DateTime::<chrono::Utc>::from)
            .map_err(|_| "파일 생성 시각을 확인하지 못했습니다".to_string())?,
    };
    Ok((number_time.to_utc() - actual)
        .num_milliseconds()
        .unsigned_abs()
        > 12 * 60 * 60 * 1000)
}

fn check_id_time(
    root: &Path,
    resources: &Path,
    relative: &str,
    id: &str,
    context: &mut Option<IdTimeContext>,
) -> Result<bool, String> {
    if context.is_none() {
        *context = Some(id_time_context(root, resources)?);
    }
    id_time_mismatch_with_context(
        root,
        context.as_ref().expect("context initialized"),
        relative,
        id,
    )
}

#[cfg(test)]
fn id_time_mismatch(
    root: &Path,
    resources: &Path,
    relative: &str,
    id: &str,
) -> Result<bool, String> {
    id_time_mismatch_with_context(root, &id_time_context(root, resources)?, relative, id)
}

pub(crate) fn check(root: &Path, resources: &Path) -> Result<Vec<ArchiveIssue>, String> {
    check_selected(root, resources, None)
}

fn check_selected(
    root: &Path,
    resources: &Path,
    selected: Option<&str>,
) -> Result<Vec<ArchiveIssue>, String> {
    let mut issues = Vec::new();
    let mut time_context = None;
    for name in RULE_FILES {
        if !root.join(name).is_file() {
            issues.push(ArchiveIssue {
                file: name.to_string(),
                message: "규칙 파일이 없습니다".into(),
            });
        }
    }
    for (folder, is_zet) in [("zettels", true), ("receipts", false)] {
        for entry in fs::read_dir(root.join(folder))
            .map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?
        {
            let entry = entry.map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let file = format!("{folder}/{name}");
            if selected.is_some_and(|selected| selected != file) {
                continue;
            }
            let issue = |message: String| ArchiveIssue {
                file: file.clone(),
                message,
            };
            if !entry
                .file_type()
                .map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?
                .is_file()
                || entry.path().extension().is_none_or(|x| x != "md")
            {
                issues.push(issue("마크다운 파일이 아니거나 하위 폴더입니다".into()));
                continue;
            }
            let text = match fs::read_to_string(entry.path()) {
                Ok(value) => value,
                Err(_) => {
                    issues.push(issue("파일 내용을 읽지 못했습니다".into()));
                    continue;
                }
            };
            if is_zet {
                match crate::archive_zet::parse(&text) {
                    Ok((head, _)) => {
                        if check_id_time(root, resources, &file, &head.id, &mut time_context)? {
                            issues.push(issue("번호의 시각이 실제와 많이 다름".into()));
                        }
                        if name != format!("{}.md", head.id) {
                            issues.push(issue("글 번호와 파일 이름이 다릅니다".into()));
                        }
                        for asset in &head.assets {
                            let hash = &asset[7..];
                            if !root
                                .join("objects")
                                .join("sha256")
                                .join(&hash[..2])
                                .join(hash)
                                .is_file()
                            {
                                issues.push(issue(format!("원본을 찾을 수 없습니다: {asset}")));
                            }
                        }
                        for edge in &head.edges {
                            if edge.target == head.id
                                || !root
                                    .join("zettels")
                                    .join(format!("{}.md", edge.target))
                                    .is_file()
                            {
                                issues.push(issue(format!(
                                    "연결된 다른 글을 찾을 수 없습니다: {}",
                                    edge.target
                                )));
                            }
                        }
                    }
                    Err(message) => issues.push(issue(message)),
                }
            } else {
                match crate::archive_receipt::parse(&text) {
                    Ok((head, _)) => {
                        if check_id_time(root, resources, &file, &head.id, &mut time_context)? {
                            issues.push(issue("번호의 시각이 실제와 많이 다름".into()));
                        }
                        if name != format!("{}.md", head.id) {
                            issues.push(issue("작업 기록 번호와 파일 이름이 다릅니다".into()));
                        }
                    }
                    Err(message) => issues.push(issue(message)),
                }
            }
        }
    }
    issues.sort_by(|a, b| a.file.cmp(&b.file).then(a.message.cmp(&b.message)));
    Ok(issues)
}

fn save_message(titles: &[String]) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    if titles.is_empty() {
        return "저장: 작업 기록".into();
    }
    let first: Vec<&str> = titles[0].graphemes(true).collect();
    if first.len() > 60 {
        let truncated = first.into_iter().take(57).collect::<String>();
        let suffix = if titles.len() > 1 {
            format!(" 외 {}건", titles.len() - 1)
        } else {
            String::new()
        };
        return format!("저장: {truncated}…{suffix}");
    }
    let all = titles.join(", ");
    if all.graphemes(true).count() <= 60 {
        return format!("저장: {all}");
    }
    for keep in (1..titles.len()).rev() {
        let message = format!("{} 외 {}건", titles[..keep].join(", "), titles.len() - keep);
        if message.graphemes(true).count() <= 60 {
            return format!("저장: {message}");
        }
    }
    format!("저장: 외 {}건", titles.len())
}

pub(crate) fn save(root: &Path, resources: &Path, database: &Path) -> Result<(), String> {
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "저장을 완료하지 못했습니다".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    save_unlocked(root, resources, database)
}

fn save_unlocked(root: &Path, resources: &Path, database: &Path) -> Result<(), String> {
    if !check(root, resources)?.is_empty() {
        return Err("몸소 폴더 검사를 먼저 확인해 주세요".into());
    }
    let proof_key = crate::chat_archive_tracking::proof_key().ok();
    let credentials = crate::credentials::try_prevent_secret_changes().ok();
    let pending = match (credentials.as_ref(), proof_key.as_deref()) {
        (Some(_), Some(key)) => {
            crate::chat_archive_store::redaction_pending_with_key(root, database, key)
        }
        _ => true,
    };
    if !pending {
        crate::chat_archive_store::validate(root, database).map_err(str::to_string)?;
    }
    let (tool, _) = storage_tool(resources)?;
    if run_storage_tool(&tool, root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        let changed = run_storage_tool(
            &tool,
            root,
            &[
                "diff",
                "--name-only",
                "--diff-filter=MDT",
                "--no-renames",
                "HEAD",
                "--",
                "receipts/",
            ],
        )?;
        if !changed.trim().is_empty() {
            return Err("이미 저장한 작업 기록은 바꿀 수 없습니다".into());
        }
    }
    for folder in ["zettels", "receipts"] {
        let has_files = fs::read_dir(root.join(folder))
            .map_err(|_| "몸소 폴더를 읽지 못했습니다".to_string())?
            .next()
            .is_some();
        let tracked = run_storage_tool(&tool, root, &["ls-files", "--", folder])?;
        if has_files || !tracked.trim().is_empty() {
            run_storage_tool(&tool, root, &["add", "-A", "--", folder])?;
        }
    }
    if !pending {
        let chat_tracked = run_storage_tool(&tool, root, &["ls-files", "--", "chats"])?;
        if root.join("chats").exists() || !chat_tracked.trim().is_empty() {
            run_storage_tool(&tool, root, &["add", "-A", "--", "chats"])?;
        }
    }
    run_storage_tool(
        &tool,
        root,
        &["add", "--", "AGENTS.md", "CLAUDE.md", "GEMINI.md"],
    )?;
    let dictionary_paths = crate::yoga_dictionary::paths();
    let mut add_dictionary = vec!["add", "--"];
    add_dictionary.extend(dictionary_paths.iter().map(String::as_str));
    run_storage_tool(&tool, root, &add_dictionary)?;
    let mut diff_paths = vec![
        "diff",
        "--cached",
        "--name-only",
        "--no-renames",
        "--",
        "zettels/",
        "receipts/",
        "AGENTS.md",
        "CLAUDE.md",
        "GEMINI.md",
    ];
    if !pending {
        diff_paths.push("chats/");
    }
    diff_paths.extend(dictionary_paths.iter().map(String::as_str));
    let changed = run_storage_tool(&tool, root, &diff_paths)?;
    let paths: Vec<String> = changed.lines().map(str::to_owned).collect();
    if paths.is_empty() {
        return Err("저장할 변경 사항이 없습니다".into());
    }
    let mut titles = Vec::new();
    for path in &paths {
        if let Some(name) = path.strip_prefix("zettels/") {
            let id = name
                .strip_suffix(".md")
                .ok_or_else(|| "글 파일 이름이 올바르지 않습니다".to_string())?;
            if !crate::archive_zet::valid_zet_id(id) {
                return Err("글 파일 이름이 올바르지 않습니다".into());
            }
            let text = match fs::read_to_string(root.join(path)) {
                Ok(text) => text,
                Err(_) => run_storage_tool(&tool, root, &["show", &format!("HEAD:{path}")])?,
            };
            titles.push(crate::archive_zet::parse(&text)?.0.title);
        }
    }
    let message = save_message(&titles);
    let indexed_paths: Vec<String> = paths
        .iter()
        .filter(|path| path.starts_with("zettels/") || path.starts_with("receipts/"))
        .cloned()
        .collect();
    index_changed(root, database, &indexed_paths)?;
    commit_paths(&tool, root, &message, &paths)?;
    Ok(())
}

pub(crate) fn save_one(
    root: &Path,
    resources: &Path,
    database: &Path,
    id: &str,
    changes: crate::archive_zet::ZetUpdate,
    expected_hash: Option<&str>,
) -> Result<(), String> {
    use std::io::Write;
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "글을 저장하지 못했습니다".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    if !crate::archive_zet::valid_zet_id(id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    let relative = format!("zettels/{id}.md");
    let path = root.join(&relative);
    let before = fs::read_to_string(&path).map_err(|_| "글을 읽지 못했습니다".to_string())?;
    if expected_hash.is_some_and(|hash| hash != crate::archive_zet::content_hash(&before)) {
        return Err("이 글이 그사이 바뀌었습니다".into());
    }
    let (tool, _) = storage_tool(resources)
        .map_err(|_| "이력 저장 실패: 저장 도구를 찾지 못했습니다".to_string())?;
    let original_index =
        run_storage_tool(&tool, root, &["ls-files", "--stage", "--", &relative])
            .map_err(|_| "이력 저장 실패: 저장 상태를 확인하지 못했습니다".to_string())?;
    if original_index.lines().count() > 1
        || original_index
            .lines()
            .next()
            .is_some_and(|line| !line.contains(" 0\t"))
    {
        return Err("이력 저장 실패: 글의 저장 상태를 확인하지 못했습니다".into());
    }
    let update_result =
        crate::archive_zet::update_inner(root, database, id, changes, expected_hash);
    if let Err(error) = update_result {
        if error == "이 글이 그사이 바뀌었습니다" {
            return Err(error);
        }
        match fs::read_to_string(&path) {
            Err(_) => return Err(format!("글 저장 실패: {error}; 글을 되돌리지 못했습니다")),
            Ok(current) if current != before => {
                if fs::write(&path, before.as_bytes()).is_err()
                    || index_changed(root, database, std::slice::from_ref(&relative)).is_err()
                {
                    return Err(format!("글 저장 실패: {error}; 글을 되돌리지 못했습니다"));
                }
            }
            Ok(_) => {}
        }
        return Err(format!("글 저장 실패: {error}"));
    }
    let written = fs::read_to_string(&path).map_err(|_| "글을 저장하지 못했습니다".to_string())?;
    let result = (|| -> Result<(), String> {
        let issues = check_selected(root, resources, Some(&relative))?;
        if issues.iter().any(|issue| issue.file == relative) {
            return Err("글의 규칙을 확인해 주세요".into());
        }
        run_storage_tool(&tool, root, &["add", "--", &relative])?;
        if fs::read_to_string(&path).map_err(|_| "글을 읽지 못했습니다".to_string())? != written
        {
            return Err("이 글이 그사이 바뀌었습니다".into());
        }
        let (head, _) = crate::archive_zet::parse(&written)?;
        let message = save_message(&[head.title]);
        run_storage_tool(
            &tool,
            root,
            &[
                "-c",
                "user.name=momso",
                "-c",
                "user.email=archive@momso.local",
                "commit",
                "--only",
                "-m",
                &message,
                "--",
                &relative,
            ],
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        let restore_index = || -> Result<(), String> {
            if let Some(line) = original_index.lines().next() {
                let (metadata, _) = line
                    .split_once('\t')
                    .ok_or_else(|| "저장 상태 복원 실패".to_string())?;
                let parts: Vec<_> = metadata.split_whitespace().collect();
                if parts.len() != 3 {
                    return Err("저장 상태 복원 실패".into());
                }
                run_storage_tool(
                    &tool,
                    root,
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        parts[0],
                        parts[1],
                        &relative,
                    ],
                )?;
            } else {
                run_storage_tool(
                    &tool,
                    root,
                    &["update-index", "--force-remove", "--", &relative],
                )?;
            }
            Ok(())
        };
        let index_restored = restore_index().is_ok();
        let mut file_restored = false;
        if fs::read_to_string(&path).is_ok_and(|current| current == written) {
            if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(root.join("zettels")) {
                if tmp.write_all(before.as_bytes()).is_ok()
                    && tmp.as_file().sync_all().is_ok()
                    && tmp.persist(&path).is_ok()
                {
                    file_restored =
                        index_changed(root, database, std::slice::from_ref(&relative)).is_ok();
                }
            }
        }
        if !file_restored || !index_restored {
            return Err(format!(
                "이력 저장 실패: {error}; 글 또는 저장 상태를 되돌리지 못했습니다"
            ));
        }
        return Err(format!("이력 저장 실패: {error}; 글을 원래대로 돌렸습니다"));
    }
    Ok(())
}

fn index_changed(root: &Path, database: &Path, paths: &[String]) -> Result<(), String> {
    let mut conn = crate::storage::open_writable_connection(database)
        .map_err(|_| "색인을 열지 못했습니다".to_string())?;
    let tx = conn
        .transaction()
        .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
    for path in paths {
        let (folder, name) = path
            .split_once('/')
            .ok_or_else(|| "파일 경로가 올바르지 않습니다".to_string())?;
        let id = name
            .strip_suffix(".md")
            .ok_or_else(|| "파일 이름이 올바르지 않습니다".to_string())?;
        if folder == "zettels" {
            if !crate::archive_zet::valid_zet_id(id) {
                return Err("글 번호가 올바르지 않습니다".into());
            }
            tx.execute("DELETE FROM archive_zets_assets WHERE zet_id=?1", [id])
                .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
            tx.execute("DELETE FROM archive_zets_edges WHERE zet_id=?1", [id])
                .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
            tx.execute("DELETE FROM archive_zets WHERE id=?1", [id])
                .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
            if root.join(path).is_file() {
                let text = fs::read_to_string(root.join(path))
                    .map_err(|_| "글을 읽지 못했습니다".to_string())?;
                let (h, _) = crate::archive_zet::parse(&text)?;
                if h.id != id {
                    return Err("글 번호와 파일 이름이 다릅니다".into());
                }
                tx.execute("INSERT INTO archive_zets(id,title,summary,kind,status,created_at,updated_at,file_path) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)", rusqlite::params![h.id,h.title,h.summary,h.kind,h.status,h.created_at,h.updated_at,path]).map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
                for asset in &h.assets {
                    tx.execute(
                        "INSERT OR IGNORE INTO archive_zets_assets(zet_id,asset_id) VALUES (?1,?2)",
                        rusqlite::params![h.id, asset],
                    )
                    .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
                }
                for edge in &h.edges {
                    tx.execute("INSERT OR IGNORE INTO archive_zets_edges(zet_id,edge_type,target_id) VALUES (?1,?2,?3)", rusqlite::params![h.id,edge.kind,edge.target]).map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
                }
            }
        } else if folder == "receipts" {
            if !crate::archive_receipt::valid_id(id) {
                return Err("작업 기록 번호가 올바르지 않습니다".into());
            }
            tx.execute(
                "DELETE FROM archive_receipts_read WHERE receipt_id=?1",
                [id],
            )
            .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
            tx.execute(
                "DELETE FROM archive_receipts_wrote WHERE receipt_id=?1",
                [id],
            )
            .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
            tx.execute("DELETE FROM archive_receipts WHERE id=?1", [id])
                .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
            if root.join(path).is_file() {
                let text = fs::read_to_string(root.join(path))
                    .map_err(|_| "작업 기록을 읽지 못했습니다".to_string())?;
                let (h, _) = crate::archive_receipt::parse(&text)?;
                if h.id != id {
                    return Err("작업 기록 번호와 파일 이름이 다릅니다".into());
                }
                tx.execute(
                    "INSERT INTO archive_receipts(id,created_at,tool,model) VALUES (?1,?2,?3,?4)",
                    rusqlite::params![h.id, h.created_at, h.tool, h.model],
                )
                .map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
                for reference in &h.read {
                    tx.execute("INSERT OR IGNORE INTO archive_receipts_read(receipt_id,reference_id) VALUES (?1,?2)", rusqlite::params![h.id,reference]).map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
                }
                for reference in &h.wrote {
                    tx.execute("INSERT OR IGNORE INTO archive_receipts_wrote(receipt_id,reference_id) VALUES (?1,?2)", rusqlite::params![h.id,reference]).map_err(|_| "색인을 갱신하지 못했습니다".to_string())?;
                }
            }
        } else {
            return Err("파일 경로가 올바르지 않습니다".into());
        }
    }
    tx.commit()
        .map_err(|_| "색인을 갱신하지 못했습니다".to_string())
}

pub(crate) fn restore_version(
    root: &Path,
    resources: &Path,
    database: &Path,
    id: &str,
    revision: &str,
    discard_unsaved: bool,
) -> Result<(), String> {
    use std::io::Write;
    let relative = format!("zettels/{id}.md");
    if !crate::archive_zet::valid_zet_id(id) {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    let _guard = crate::archive_zet::WRITE_LOCK
        .lock()
        .map_err(|_| "이전 글을 복원하지 못했습니다".to_string())?;
    crate::archive_remote::ensure_document_write_allowed(root, database)?;
    let (tool, _) = storage_tool(resources)?;
    let pending = run_storage_tool(&tool, root, &["status", "--porcelain", "--", &relative])?;
    if !discard_unsaved && !pending.trim().is_empty() {
        return Err("저장하지 않은 내용이 있습니다".into());
    }
    let old = read_version(root, resources, id, revision)?;
    let (mut restored, old_body) = crate::archive_zet::parse(&old)?;
    let path = root.join("zettels").join(format!("{id}.md"));
    let current_text =
        fs::read_to_string(&path).map_err(|_| "현재 글을 읽지 못했습니다".to_string())?;
    let (current, _) = crate::archive_zet::parse(&current_text)?;
    if restored.id != id || current.id != id {
        return Err("글 번호가 올바르지 않습니다".into());
    }
    restored.created_at = current.created_at;
    restored.updated_at = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let text = crate::archive_zet::render(&restored, old_body)?;
    let mut temporary = tempfile::NamedTempFile::new_in(root.join("zettels"))
        .map_err(|_| "이전 글을 복원하지 못했습니다".to_string())?;
    temporary
        .write_all(text.as_bytes())
        .map_err(|_| "이전 글을 복원하지 못했습니다".to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| "이전 글을 복원하지 못했습니다".to_string())?;
    temporary
        .persist(path)
        .map_err(|_| "이전 글을 복원하지 못했습니다".to_string())?;
    save_unlocked(root, resources, database)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn commit_paths_keeps_unrelated_staged_files_and_handles_literal_names() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (tool, _) = storage_tool(root).unwrap();
        run_storage_tool(&tool, root, &["init", "-b", "main"]).unwrap();
        fs::write(root.join("literal[1].txt"), "selected").unwrap();
        fs::write(root.join("other.txt"), "staged").unwrap();
        run_storage_tool(
            &tool,
            root,
            &[
                "--literal-pathspecs",
                "add",
                "--",
                "literal[1].txt",
                "other.txt",
            ],
        )
        .unwrap();
        commit_paths(&tool, root, "fixture", &["literal[1].txt".into()]).unwrap();
        assert_eq!(
            run_storage_tool(&tool, root, &["show", "HEAD:literal[1].txt"]).unwrap(),
            "selected"
        );
        assert!(run_storage_tool(&tool, root, &["show", "HEAD:other.txt"]).is_err());
        assert_eq!(
            run_storage_tool(&tool, root, &["diff", "--cached", "--name-only"])
                .unwrap()
                .trim(),
            "other.txt"
        );
    }

    #[test]
    fn batched_additions_match_follow_history_and_renames_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (tool, _) = storage_tool(root).unwrap();
        run_storage_tool(&tool, root, &["init", "-b", "main"]).unwrap();
        fs::create_dir(root.join("zettels")).unwrap();
        fs::create_dir(root.join("receipts")).unwrap();
        let file = "zettels/zet_20200101_000000.md";
        let commit = |message: &str, date: &str| {
            let status = Command::new(&tool)
                .args([
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "-m",
                    message,
                ])
                .current_dir(root)
                .env("GIT_CONFIG_GLOBAL", "NUL")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_COMMITTER_DATE", date)
                .env("GIT_AUTHOR_DATE", date)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        };
        fs::write(root.join(file), "first").unwrap();
        run_storage_tool(&tool, root, &["add", "--", file]).unwrap();
        commit("add", "2020-01-01T00:00:00+00:00");
        fs::remove_file(root.join(file)).unwrap();
        run_storage_tool(&tool, root, &["add", "-A", "--", file]).unwrap();
        commit("remove", "2020-01-02T00:00:00+00:00");
        fs::write(root.join(file), "again").unwrap();
        run_storage_tool(&tool, root, &["add", "--", file]).unwrap();
        commit("readd", "2020-01-03T00:00:00+00:00");
        let context = id_time_context(root, root).unwrap();
        let batched = context.additions.as_ref().unwrap().get(file).unwrap();
        let followed = run_storage_tool(
            &tool,
            root,
            &[
                "log",
                "--follow",
                "--diff-filter=A",
                "--reverse",
                "--format=%cI",
                "HEAD",
                "--",
                file,
            ],
        )
        .unwrap();
        let first = chrono::DateTime::parse_from_rfc3339(followed.lines().next().unwrap())
            .unwrap()
            .to_utc();
        assert_eq!(*batched, first);
        let renamed = "zettels/zet_20200101_000001.md";
        run_storage_tool(&tool, root, &["mv", "--", file, renamed]).unwrap();
        commit("rename", "2020-01-04T00:00:00+00:00");
        assert!(id_time_context(root, root).unwrap().additions.is_none());
    }
    #[test]
    fn reindex_waits_until_document_writer_releases_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        let db = temp.path().join("momso.db");
        initialize(&root, temp.path()).unwrap();
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let guard = crate::archive_zet::WRITE_LOCK.lock().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            done_tx.send(reindex(&root, &db).is_ok()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        drop(guard);
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap());
        worker.join().unwrap();
    }
    #[test]
    fn document_and_receipt_creation_wait_for_reindex_lock() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        let db = temp.path().join("momso.db");
        initialize(&root, temp.path()).unwrap();
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let guard = crate::archive_zet::WRITE_LOCK.lock().unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let doc_root = root.clone();
        let doc_db = db.clone();
        let doc_started = started_tx.clone();
        let doc_done = done_tx.clone();
        let document = std::thread::spawn(move || {
            doc_started.send(()).unwrap();
            let result = crate::archive_zet::create(
                &doc_root,
                &doc_db,
                crate::archive_zet::ZetCreate {
                    title: "Lesson".into(),
                    summary: "Summary".into(),
                    kind: "note".into(),
                    body: "Body".into(),
                    assets: vec![],
                    edges: vec![],
                },
            );
            doc_done.send(result.is_ok()).unwrap();
        });
        let receipt_root = root.clone();
        let receipt_db = db.clone();
        let receipt = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let result = crate::archive_receipt::create(
                &receipt_root,
                &receipt_db,
                crate::archive_receipt::ReceiptCreate {
                    tool: "person".into(),
                    model: String::new(),
                    chat: None,
                    read: vec![],
                    wrote: vec![],
                    prompt: String::new(),
                    changes: String::new(),
                },
            );
            done_tx.send(result.is_ok()).unwrap();
        });
        started_rx.recv().unwrap();
        started_rx.recv().unwrap();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err());
        drop(guard);
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap());
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap());
        document.join().unwrap();
        receipt.join().unwrap();
        let result = reindex(&root, &db).unwrap();
        assert_eq!((result.zets, result.receipts), (1, 1));
    }
    #[test]
    fn managed_rule_choice_keeps_profile_and_rejects_custom_text() {
        let old = include_str!("testdata/archive_rules_before_dictionary.md");
        let current = ARCHIVE_RULES;
        assert_eq!(select_managed_rule(old, current), Some(true));
        assert_eq!(select_managed_rule(current, old), Some(false));
        assert_eq!(select_managed_rule("custom", current), None);
        let profile = "\n## 사용자에 대해\n\n사용자 정보가 몸소 보관함 규칙과 어긋나면 몸소 보관함 규칙을 먼저 따른다.\n\n### 이름\n\nExample\n";
        assert_eq!(
            select_managed_rule(&format!("{old}{profile}"), &format!("{current}{profile}")),
            Some(true)
        );
        assert_eq!(
            select_managed_rule(&format!("{old}{profile}"), current),
            None
        );
    }
    #[test]
    fn save_message_counts_visible_characters_and_shortens_long_first_title() {
        let long = "가".repeat(61);
        assert_eq!(
            save_message(std::slice::from_ref(&long)),
            format!("저장: {}…", "가".repeat(57))
        );
        assert_eq!(
            save_message(&[long, "다음".into()]),
            format!("저장: {}… 외 1건", "가".repeat(57))
        );
        let combining = "e\u{301}".repeat(61);
        assert_eq!(
            save_message(&[combining]),
            format!("저장: {}…", "e\u{301}".repeat(57))
        );
        assert_eq!(
            save_message(&["가".repeat(55), "나".repeat(10)]),
            format!("저장: {} 외 1건", "가".repeat(55))
        );
    }
    #[test]
    fn resource_directory_is_not_used_as_a_git_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("mingit/cmd");
        fs::create_dir_all(&directory).unwrap();
        let bundled = directory.join("git.exe");
        fs::write(&bundled, b"fixture").unwrap();
        assert_ne!(
            storage_tool(temp.path()).ok().map(|(path, _)| path),
            Some(bundled)
        );
    }
    #[test]
    fn same_second_waits_before_allocating_next_number() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("zet_20261115_143052.md"), "보존").unwrap();
        let mut tick = 0;
        let mut waited = 0;
        let (id, _, _file) = allocate_with_clock(
            temp.path(),
            "zet",
            || {
                tick += 1;
                if tick == 1 {
                    ("20261115_143052".into(), "2026-11-15T14:30:52+09:00".into())
                } else {
                    ("20261115_143053".into(), "2026-11-15T14:30:53+09:00".into())
                }
            },
            || {
                waited += 1;
            },
        )
        .unwrap();
        assert_eq!(waited, 1);
        assert_eq!(id, "zet_20261115_143053");
        assert_eq!(
            fs::read_to_string(temp.path().join("zet_20261115_143052.md")).unwrap(),
            "보존"
        );
    }
    #[test]
    #[ignore = "수동 보관함 시험 도구"]
    fn trial_archive_driver() {
        let root = std::path::PathBuf::from(
            std::env::var_os("MOMSO_TRIAL_ARCHIVE").expect("시험 보관함 경로"),
        );
        if std::env::var_os("MOMSO_TRIAL_CHECK_ONLY").is_none() {
            initialize(&root, &root).unwrap();
        }
        let issues = check(&root, &root).unwrap();
        println!(
            "MOMSO_TRIAL_ISSUES={}",
            serde_json::to_string(&issues).unwrap()
        );
    }

    fn prior_language_rules() -> String {
        before_skills_rules().replace(
            "- 사람에게 말할 때 보관함의 글은 \"문서\", 바뀌지 않는 원래 파일은 \"원자료\", AI가 한 일의 기록은 \"작업 내역\", AI와 주고받는 곳은 \"채팅방\", 이력에 남기는 일은 \"저장\"이라고 한다. 문서를 가리킬 때는 파일 이름이나 경로가 아니라 그 문서의 제목으로 말한다.\n- 사람에게 \"zet\", \"제텔\", \"오브젝트\", \"영수증\", \"커밋\", \"git\", \"frontmatter\", \"세션\", \"스레드\"라고 말하지 않는다.",
            "- 사람에게 말할 때 \"커밋\", \"git\", \"frontmatter\", \"zet\" 같은 말을 쓰지 않는다. \"저장\", \"지난 판\", \"글\", \"원본\", \"작업 기록\"이라고 한다. 글을 가리킬 때는 파일 이름이나 경로가 아니라 그 글의 제목으로 말한다."
        )
    }

    fn previous_speaker_rules() -> String {
        let prior = prior_language_rules();
        let start = prior.find("- 보정한 전사에서 화자를").unwrap();
        let end = prior[start..].find("\n\n## 작업 기록").unwrap() + start;
        format!("{}- 누가 한 말인지 표시가 없으면 내용으로 추정해 `이름(추정)` 으로 적는다. 단서가 전혀 없을 때만 `화자 불명` 으로 둔다.{}", &prior[..start], &prior[end..])
    }

    fn before_skills_rules() -> String {
        include_str!("testdata/archive_rules_before_dictionary.md").replace("\r\n", "\n").replace("- `skills/` 와 `.agents/skills/`, `.claude/skills/` 는 몸소가 관리한다. AI는 사람이 시켰을 때만 스킬을 고친다.\n", "")
    }

    #[test]
    fn upgrades_suffix_question_rules_and_preserves_custom_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let previous = prior_language_rules()
            .replace("`?이름`", "`이름?`")
            .replace("`?김성균:", "`김성균?:")
            .replace("이름 앞의 `?`", "이름 뒤의 `?`");
        use sha2::{Digest, Sha256};
        for contents in [previous.clone(), previous.replace('\n', "\r\n")] {
            assert!(PREVIOUS_RULE_HASHES
                .contains(&format!("{:x}", Sha256::digest(contents.as_bytes())).as_str()));
        }
        fs::write(root.join("AGENTS.md"), previous.as_bytes()).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            previous.replace('\n', "\r\n").as_bytes(),
        )
        .unwrap();
        fs::write(root.join("GEMINI.md"), "사람이 수정한 규칙").unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            ARCHIVE_RULES
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            ARCHIVE_RULES
        );
        assert_eq!(
            fs::read_to_string(root.join("GEMINI.md")).unwrap(),
            "사람이 수정한 규칙"
        );
        assert!(status(&root, temp.path()).unwrap().rules_modified);
        assert!(!ARCHIVE_RULES.contains("이름?"));
        assert!(!ARCHIVE_RULES.contains("(추정)"));
        assert!(ARCHIVE_RULES.contains("?김성균:"));
        assert!(ARCHIVE_RULES.contains("말의 내용에 있는 물음표는 건드리지 않는다"));
    }

    #[test]
    fn upgrades_previous_dictionary_rules_preserving_custom_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let previous =
            include_str!("testdata/archive_rules_before_spelling_policy.md").replace("\r\n", "\n");
        fs::write(root.join("AGENTS.md"), &previous).unwrap();
        fs::write(root.join("CLAUDE.md"), &previous).unwrap();
        fs::write(root.join("GEMINI.md"), "custom rules").unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            ARCHIVE_RULES.replace("\r\n", "\n")
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            ARCHIVE_RULES.replace("\r\n", "\n")
        );
        assert_eq!(
            fs::read_to_string(root.join("GEMINI.md")).unwrap(),
            "custom rules"
        );
    }

    #[test]
    fn upgrades_rules_before_fragment_lookup_without_touching_custom_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let old = include_str!("testdata/archive_rules_before_fragments.md").replace("\r\n", "\n");
        fs::write(root.join("AGENTS.md"), &old).unwrap();
        fs::write(root.join("CLAUDE.md"), "custom rules").unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            ARCHIVE_RULES.replace("\r\n", "\n")
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "custom rules"
        );
    }

    #[test]
    fn upgrades_previous_speaker_rules_without_overwriting_custom_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let previous = previous_speaker_rules();
        use sha2::{Digest, Sha256};
        for contents in [previous.clone(), previous.replace('\n', "\r\n")] {
            assert!(PREVIOUS_RULE_HASHES
                .contains(&format!("{:x}", Sha256::digest(contents.as_bytes())).as_str()));
        }
        fs::write(root.join("AGENTS.md"), previous.as_bytes()).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            previous.replace('\n', "\r\n").as_bytes(),
        )
        .unwrap();
        fs::write(root.join("GEMINI.md"), "사람이 수정한 규칙").unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            ARCHIVE_RULES
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            ARCHIVE_RULES
        );
        assert_eq!(
            fs::read_to_string(root.join("GEMINI.md")).unwrap(),
            "사람이 수정한 규칙"
        );
        assert!(status(&root, temp.path()).unwrap().rules_modified);
        assert!(!ARCHIVE_RULES.contains("(추정)"));
        assert!(ARCHIVE_RULES.contains("?김성균:"));
        assert!(ARCHIVE_RULES.contains("말의 내용에 있는 물음표는 건드리지 않는다"));
    }

    #[test]
    fn upgrades_exact_previous_rules_and_preserves_custom_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let previous_rules = previous_speaker_rules();
        let previous = previous_rules.lines().map(|line| {
            if line.starts_with("- 파일 이름: `zettels/") { "- 파일 이름: `zettels/zet_YYYYMMDD_HHMMSS.md`. 지금 이 컴퓨터의 날짜와 시각이다. 그 이름의 파일이 이미 있으면 1초 뒤의 시각으로 만든다. 이름 뒤에 다른 것을 붙이지 않는다. 한글이나 영어 낱말을 넣지 않는다.".to_string() }
            else if line.starts_with("- 글을 만들거나 고치는 일을") { line.replace(" 시각을 정하는 방법도 글과 같다.", "") }
            else if line.starts_with("- 사람에게 말할 때") { line.replace(" 글을 가리킬 때는 파일 이름이나 경로가 아니라 그 글의 제목으로 말한다.", "") }
            else { line.to_string() }
        }).collect::<Vec<_>>().join("\n") + "\n";
        use sha2::{Digest, Sha256};
        assert!(PREVIOUS_RULE_HASHES
            .contains(&format!("{:x}", Sha256::digest(previous.as_bytes())).as_str()));
        fs::write(root.join("AGENTS.md"), previous.as_bytes()).unwrap();
        fs::write(
            root.join("CLAUDE.md"),
            previous.replace('\n', "\r\n").as_bytes(),
        )
        .unwrap();
        fs::write(root.join("GEMINI.md"), "사람이 고친 규칙").unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            ARCHIVE_RULES
        );
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            ARCHIVE_RULES
        );
        assert_eq!(
            fs::read_to_string(root.join("GEMINI.md")).unwrap(),
            "사람이 고친 규칙"
        );
        assert!(status(&root, temp.path()).unwrap().rules_modified);
    }

    #[test]
    fn automatic_rule_update_saves_only_clean_known_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let previous = prior_language_rules();
        for name in RULE_FILES {
            fs::write(root.join(name), previous.as_bytes()).unwrap();
        }
        let (tool, _) = storage_tool(temp.path()).unwrap();
        run_storage_tool(
            &tool,
            &root,
            &["add", "--", "AGENTS.md", "CLAUDE.md", "GEMINI.md"],
        )
        .unwrap();
        run_storage_tool(
            &tool,
            &root,
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .unwrap();
        fs::write(root.join("user-staged.txt"), "keep staged").unwrap();
        run_storage_tool(&tool, &root, &["add", "--", "user-staged.txt"]).unwrap();
        fs::write(
            root.join("profile.md"),
            "---\nname: ''\nabout: ''\nstyle: ''\n---\n",
        )
        .unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read(root.join("AGENTS.md")).unwrap(),
            previous.as_bytes()
        );
        fs::remove_file(root.join("profile.md")).unwrap();
        initialize(&root, temp.path()).unwrap();
        assert!(!status(&root, temp.path()).unwrap().has_unsaved_changes);
        let message = run_storage_tool(
            &tool,
            &root,
            &["log", "-1", "--format=%s", "--", "AGENTS.md"],
        )
        .unwrap();
        assert_eq!(message.trim(), "몸소 업데이트: 기본 규칙 갱신");
        let staged = run_storage_tool(&tool, &root, &["diff", "--cached", "--name-only"]).unwrap();
        assert!(staged.lines().any(|line| line == "user-staged.txt"));
        let saved =
            run_storage_tool(&tool, &root, &["show", "--format=", "--name-only", "HEAD"]).unwrap();
        assert!(!saved.lines().any(|line| line == "user-staged.txt"));
        fs::write(root.join("CLAUDE.md"), previous.as_bytes()).unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read(root.join("CLAUDE.md")).unwrap(),
            previous.as_bytes()
        );
        assert!(status(&root, temp.path()).unwrap().has_unsaved_changes);
    }

    #[test]
    fn timestamp_checks_creation_and_first_saved_version() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let current = chrono::Local::now();
        let old = current - chrono::Duration::hours(13);
        let now_id = format!("zet_{}", current.format("%Y%m%d_%H%M%S"));
        let old_id = format!("rec_{}", old.format("%Y%m%d_%H%M%S"));
        let now_relative = format!("zettels/{now_id}.md");
        let old_relative = format!("receipts/{old_id}.md");
        let stamp = current.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let zet = format!("---\nid: {now_id}\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '{stamp}'\nupdated_at: '{stamp}'\nassets: []\nedges: []\n---\n본문\n");
        let receipt = format!("---\nid: {old_id}\ncreated_at: '{stamp}'\ntool: person\nmodel: test\nread: []\nwrote: []\n---\n## 시킨 말\n없음\n## 고친 내용\n없음\n");
        fs::write(root.join(&now_relative), &zet).unwrap();
        fs::write(root.join(&old_relative), receipt).unwrap();
        assert!(!id_time_mismatch(&root, temp.path(), &now_relative, &now_id).unwrap());
        let problems = check(&root, temp.path()).unwrap();
        assert!(problems
            .iter()
            .any(|issue| issue.file == old_relative
                && issue.message == "번호의 시각이 실제와 많이 다름"));
        let (tool, _) = storage_tool(temp.path()).unwrap();
        run_storage_tool(&tool, &root, &["add", "--", &now_relative]).unwrap();
        let old_stamp = old.to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let output = std::process::Command::new(&tool)
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "NUL")
            .env("GIT_COMMITTER_DATE", &old_stamp)
            .args([
                "-c",
                "user.name=momso",
                "-c",
                "user.email=archive@momso.local",
                "commit",
                "--date",
                &old_stamp,
                "-m",
                "fixture",
                "--",
                &now_relative,
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(id_time_mismatch(&root, temp.path(), &now_relative, &now_id).unwrap());
        assert!(check(&root, temp.path())
            .unwrap()
            .iter()
            .any(|issue| issue.file == now_relative
                && issue.message == "번호의 시각이 실제와 많이 다름"));
    }

    #[test]
    fn panel_save_checks_hash_isolates_history_and_rolls_back_failure() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let create = |title: &str| {
            crate::archive_zet::create(
                &root,
                &db,
                crate::archive_zet::ZetCreate {
                    title: title.into(),
                    summary: "요약".into(),
                    kind: "note".into(),
                    body: "원본 본문".into(),
                    assets: vec![],
                    edges: vec![],
                },
            )
            .unwrap()
        };
        let first = create("첫 글");
        let second = create("다른 글");
        save(&root, temp.path(), &db).unwrap();
        let changes = |body: &str| crate::archive_zet::ZetUpdate {
            title: None,
            summary: None,
            body: Some(body.into()),
            status: None,
            assets: None,
            edges: None,
        };
        let file1 = root.join("zettels").join(format!("{first}.md"));
        let file2 = root.join("zettels").join(format!("{second}.md"));
        let initial = fs::read_to_string(&file1).unwrap();
        let old_hash = crate::archive_zet::content_hash(&initial);
        crate::archive_zet::update_checked(&root, &db, &first, changes("바깥 변경"), None).unwrap();
        let edited = fs::read_to_string(&file1).unwrap();
        assert_eq!(
            crate::archive_zet::update_checked(
                &root,
                &db,
                &first,
                changes("덮어쓰기"),
                Some(&old_hash)
            )
            .unwrap_err(),
            "이 글이 그사이 바뀌었습니다"
        );
        assert_eq!(
            save_one(
                &root,
                temp.path(),
                &db,
                &first,
                changes("덮어쓰기"),
                Some(&old_hash)
            )
            .unwrap_err(),
            "이 글이 그사이 바뀌었습니다"
        );
        assert_eq!(fs::read_to_string(&file1).unwrap(), edited);
        crate::archive_zet::update_checked(
            &root,
            &db,
            &second,
            changes("다른 글의 미저장 변경"),
            None,
        )
        .unwrap();
        let (tool, _) = storage_tool(temp.path()).unwrap();
        let relative2 = format!("zettels/{second}.md");
        run_storage_tool(&tool, &root, &["add", "--", &relative2]).unwrap();
        let staged_before =
            run_storage_tool(&tool, &root, &["ls-files", "--stage", "--", &relative2]).unwrap();
        crate::archive_zet::update_checked(
            &root,
            &db,
            &second,
            changes("다른 글의 추가 미저장 변경"),
            None,
        )
        .unwrap();
        let second_before = fs::read_to_string(&file2).unwrap();
        save_one(
            &root,
            temp.path(),
            &db,
            &first,
            changes("패널 저장"),
            Some(&crate::archive_zet::content_hash(&edited)),
        )
        .unwrap();
        assert!(fs::read_to_string(&file1).unwrap().contains("패널 저장"));
        assert_eq!(fs::read_to_string(&file2).unwrap(), second_before);
        assert_eq!(
            run_storage_tool(&tool, &root, &["ls-files", "--stage", "--", &relative2]).unwrap(),
            staged_before
        );
        let files = run_storage_tool(
            &tool,
            &root,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        )
        .unwrap();
        assert_eq!(
            files.lines().collect::<Vec<_>>(),
            vec![format!("zettels/{first}.md")]
        );
        let before_failure = fs::read_to_string(&file1).unwrap();
        fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\nexit 1\n").unwrap();
        let mut failed_changes = changes("저장 실패 본문");
        failed_changes.title = Some("실패한 제목".into());
        let result = save_one(&root, temp.path(), &db, &first, failed_changes, None).unwrap_err();
        assert!(
            result.contains("이력 저장 실패") && result.contains("원래대로 돌렸습니다"),
            "{result}"
        );
        assert_eq!(fs::read_to_string(&file1).unwrap(), before_failure);
        assert_eq!(fs::read_to_string(&file2).unwrap(), second_before);
        assert_eq!(
            run_storage_tool(&tool, &root, &["ls-files", "--stage", "--", &relative2]).unwrap(),
            staged_before
        );
        let indexed = crate::storage::open_writable_connection(&db)
            .unwrap()
            .query_row(
                "SELECT title FROM archive_zets WHERE id=?1",
                [&first],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        assert_eq!(indexed, "첫 글");
    }

    #[test]
    fn creates_preserves_checks_and_saves_rules() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        for name in RULE_FILES {
            assert_eq!(fs::read(root.join(name)).unwrap(), ARCHIVE_RULES.as_bytes());
        }
        assert!(!status(&root, temp.path()).unwrap().rules_modified);
        fs::write(root.join("CLAUDE.md"), "사람이 고친 규칙").unwrap();
        initialize(&root, temp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "사람이 고친 규칙"
        );
        assert!(status(&root, temp.path()).unwrap().rules_modified);
        fs::remove_file(root.join("GEMINI.md")).unwrap();
        assert!(check(&root, temp.path())
            .unwrap()
            .iter()
            .any(|issue| issue.file == "GEMINI.md"));
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        save(&root, temp.path(), &db).unwrap();
        let (tool, _) = storage_tool(temp.path()).unwrap();
        let files =
            run_storage_tool(&tool, &root, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
        for name in RULE_FILES {
            assert!(files.lines().any(|file| file == name));
        }
        assert!(check(&root, temp.path()).unwrap().is_empty());
    }

    #[test]
    fn rule_line_endings_are_equivalent_with_and_without_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        for file in RULE_FILES {
            fs::write(root.join(file), ARCHIVE_RULES.replace('\n', "\r\n")).unwrap();
        }
        assert!(!status(&root, temp.path()).unwrap().rules_modified);
        profile_set(
            &root,
            temp.path(),
            "봄".into(),
            "첫 줄\n둘째 줄".into(),
            "".into(),
        )
        .unwrap();
        let expected = profile_rules(&profile_get(&root).unwrap());
        for file in RULE_FILES {
            fs::write(root.join(file), expected.replace('\n', "\r\n")).unwrap();
        }
        assert!(!status(&root, temp.path()).unwrap().rules_modified);
        initialize(&root, temp.path()).unwrap();
        assert!(!status(&root, temp.path()).unwrap().rules_modified);
        assert_eq!(profile_get(&root).unwrap().about, "첫 줄\n둘째 줄");
        fs::write(
            root.join("CLAUDE.md"),
            expected.replace("첫 줄", "고친 줄").replace('\n', "\r\n"),
        )
        .unwrap();
        initialize(&root, temp.path()).unwrap();
        assert!(status(&root, temp.path()).unwrap().rules_modified);
        assert!(fs::read_to_string(root.join("CLAUDE.md"))
            .unwrap()
            .contains("고친 줄"));
    }

    #[test]
    fn user_profile_updates_all_rules_and_survives_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let result = profile_set(
            &root,
            temp.path(),
            "다정".into(),
            "첫 줄\n둘째 줄: 원문".into(),
            "".into(),
        )
        .unwrap();
        assert!(result.updated_at.is_some());
        for file in RULE_FILES {
            let text = fs::read_to_string(root.join(file)).unwrap();
            assert!(text.contains("첫 줄\n둘째 줄: 원문"));
            assert!(text.contains("### 이름\n\n다정"));
            assert!(!text.contains("### 답하는 말투"));
        }
        initialize(&root, temp.path()).unwrap();
        assert_eq!(profile_get(&root).unwrap().about, "첫 줄\n둘째 줄: 원문");
        assert!(!status(&root, temp.path()).unwrap().rules_modified);
        let (tool, _) = storage_tool(temp.path()).unwrap();
        let files =
            run_storage_tool(&tool, &root, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
        let files: Vec<_> = files.lines().collect();
        for name in ["AGENTS.md", "CLAUDE.md", "GEMINI.md", "profile.md"] {
            assert!(files.contains(&name));
        }
        for (skill, _) in crate::skills::BUNDLED_SKILLS {
            for folder in ["skills", ".agents/skills", ".claude/skills"] {
                assert!(files.contains(&format!("{folder}/{skill}/SKILL.md").as_str()));
            }
        }
        assert_eq!(files.len(), 4 + 3 * crate::skills::BUNDLED_SKILLS.len());
        profile_set(&root, temp.path(), "".into(), "".into(), "".into()).unwrap();
        for file in RULE_FILES {
            assert_eq!(fs::read_to_string(root.join(file)).unwrap(), ARCHIVE_RULES);
        }
    }

    #[test]
    #[ignore = "실제 Claude 연결 시험"]
    fn live_claude_reads_user_profile() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        profile_set(
            &root,
            temp.path(),
            "푸른구름".into(),
            "".into(),
            "따뜻하고 간결하게 답해 주세요.".into(),
        )
        .unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args([
                "/C",
                "claude.cmd",
                "-p",
                "나를 뭐라고 부르면 돼?",
                "--tools",
                "",
                "--output-format",
                "json",
            ])
            .current_dir(&root)
            .env("CLAUDECODE", "")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        println!(
            "Claude 성공: {}, 이름 포함: {}",
            output.status.success(),
            text.contains("푸른구름")
        );
        assert!(
            output.status.success() && text.contains("푸른구름"),
            "Claude 응답 확인 실패"
        );
    }

    #[test]
    fn user_profile_validation_and_custom_rules_are_preserved() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        assert!(profile_set(&root, temp.path(), "줄\n바꿈".into(), "".into(), "".into()).is_err());
        assert!(profile_set(&root, temp.path(), "".into(), "가".repeat(4001), "".into()).is_err());
        fs::write(root.join("CLAUDE.md"), "사용자가 고친 내용").unwrap();
        assert!(profile_set(&root, temp.path(), "이름".into(), "".into(), "".into()).is_err());
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "사용자가 고친 내용"
        );
        assert!(!root.join("profile.md").exists());
    }

    #[test]
    fn user_profile_git_failure_restores_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let hook = root.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        let error = profile_set(&root, temp.path(), "이름".into(), "".into(), "".into())
            .err()
            .unwrap();
        assert!(error.contains("이전 상태로 돌렸습니다"));
        assert!(!root.join("profile.md").exists());
        for file in RULE_FILES {
            assert_eq!(fs::read_to_string(root.join(file)).unwrap(), ARCHIVE_RULES);
        }
        let (tool, _) = storage_tool(temp.path()).unwrap();
        assert!(run_storage_tool(
            &tool,
            &root,
            &[
                "ls-files",
                "--stage",
                "--",
                "profile.md",
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md"
            ]
        )
        .unwrap()
        .trim()
        .is_empty());
    }

    #[test]
    fn profile_and_all_rule_changes_share_one_commit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        save(&root, temp.path(), &db).unwrap();
        profile_set(
            &root,
            temp.path(),
            "fictional name".into(),
            "about".into(),
            "style".into(),
        )
        .unwrap();
        let (tool, _) = storage_tool(temp.path()).unwrap();
        let names = run_storage_tool(
            &tool,
            &root,
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"],
        )
        .unwrap();
        assert_eq!(
            names.lines().collect::<Vec<_>>(),
            vec!["AGENTS.md", "CLAUDE.md", "GEMINI.md", "profile.md"]
        );
    }

    #[test]
    fn updates_previous_rules_with_profile_without_overwriting_custom() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let profile = UserProfile {
            name: "봄".into(),
            about: "첫 줄\n둘째 줄".into(),
            style: "간결하게".into(),
            updated_at: None,
        };
        let yaml = serde_yaml::to_string(&profile).unwrap();
        let yaml = yaml.strip_prefix("---\n").unwrap_or(&yaml);
        fs::write(root.join("profile.md"), format!("---\n{yaml}---\n")).unwrap();
        let old = prior_language_rules();
        let suffix = &profile_rules(&profile)[ARCHIVE_RULES.len()..];
        for file in RULE_FILES {
            fs::write(root.join(file), format!("{old}{suffix}")).unwrap();
        }
        fs::write(root.join("CLAUDE.md"), "사용자가 고친 규칙").unwrap();
        initialize(&root, temp.path()).unwrap();
        for file in ["AGENTS.md", "GEMINI.md"] {
            assert_eq!(
                fs::read_to_string(root.join(file)).unwrap(),
                profile_rules(&profile)
            );
        }
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).unwrap(),
            "사용자가 고친 규칙"
        );
        assert!(status(&root, temp.path()).unwrap().rules_modified);
    }

    #[test]
    fn rule_edge_types_match_the_validation_list() {
        let line = ARCHIVE_RULES
            .lines()
            .find(|line| line.starts_with("- `edges` 는"))
            .unwrap();
        let values: Vec<&str> = line
            .split('`')
            .enumerate()
            .filter_map(|(i, s)| (i % 2 == 1).then_some(s))
            .filter(|s| *s != "edges" && *s != "type" && *s != "edges: []")
            .collect();
        assert_eq!(values, crate::archive_zet::EDGE_TYPES);
    }

    #[test]
    fn initializes_archive_without_tracking_originals() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        assert_eq!(fs::read(root.join(".gitignore")).unwrap(), b"objects/\n");
        fs::write(root.join("objects").join("original"), b"original").unwrap();
        assert!(status(&root, temp.path()).unwrap().has_unsaved_changes);
        fs::write(root.join("zettels").join("example.md"), "한글").unwrap();
        let result = status(&root, temp.path()).unwrap();
        assert!(result.has_unsaved_changes);
        assert_eq!(result.zet_count, 1);
    }

    #[test]
    fn reindex_discovers_hand_written_zet() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let text = "---\nid: zet_20261115_143052\ntitle: 요가 수업\nsummary: 골반 정렬\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges: []\n---\n한글 본문\n";
        fs::write(root.join("zettels/zet_20261115_143052.md"), text).unwrap();
        let result = reindex(&root, &db).unwrap();
        assert_eq!(result.zets, 1);
        assert_eq!(crate::archive_zet::list(&db, None, None).unwrap().len(), 1);
        let again = reindex(&root, &db).unwrap();
        assert_eq!(again.zets, 1);
    }

    #[test]
    fn reads_history_and_past_version_using_real_repository() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let (tool, _) = storage_tool(temp.path()).unwrap();
        let id = "zet_20261115_143052";
        fs::write(root.join("zettels").join(format!("{id}.md")), "한글 원본\n").unwrap();
        run_storage_tool(&tool, &root, &["add", "--", "zettels/"]).unwrap();
        run_storage_tool(
            &tool,
            &root,
            &[
                "-c",
                "user.name=momso",
                "-c",
                "user.email=archive@momso.local",
                "commit",
                "-m",
                "저장: 수업",
            ],
        )
        .unwrap();
        let versions = history(&root, temp.path(), id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].title, None);
        assert_eq!(
            read_version(&root, temp.path(), id, &versions[0].revision).unwrap(),
            "한글 원본\n"
        );
    }

    #[test]
    fn saves_a_zet_and_reads_its_history() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "수업".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "첫 번째".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        save(&root, temp.path(), &db).unwrap();
        let entries = history(&root, temp.path(), &id).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title.as_deref(), Some("수업"));
        assert!(read_version(&root, temp.path(), &id, &entries[0].revision)
            .unwrap()
            .contains("첫 번째"));
        assert!(!status(&root, temp.path()).unwrap().has_unsaved_changes);
        crate::archive_zet::update(
            &root,
            &db,
            &id,
            crate::archive_zet::ZetUpdate {
                title: None,
                summary: None,
                body: Some("두 번째".into()),
                status: None,
                assets: None,
                edges: None,
            },
        )
        .unwrap();
        save(&root, temp.path(), &db).unwrap();
        let path = root.join("zettels").join(format!("{id}.md"));
        let saved = fs::read_to_string(&path).unwrap();
        fs::write(&path, saved.replace("두 번째", "저장 안 한 내용")).unwrap();
        assert_eq!(
            restore_version(&root, temp.path(), &db, &id, &entries[0].revision, false).unwrap_err(),
            "저장하지 않은 내용이 있습니다"
        );
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("저장 안 한 내용"));
        restore_version(&root, temp.path(), &db, &id, &entries[0].revision, true).unwrap();
        let restored = fs::read_to_string(root.join("zettels").join(format!("{id}.md"))).unwrap();
        assert!(restored.contains("첫 번째"));
        assert_eq!(history(&root, temp.path(), &id).unwrap().len(), 3);
    }

    #[test]
    fn saves_receipt_only_without_originals() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_receipt::create(
            &root,
            &db,
            crate::archive_receipt::ReceiptCreate {
                tool: "person".into(),
                model: "".into(),
                chat: None,
                read: vec![],
                wrote: vec![],
                prompt: "".into(),
                changes: "수정".into(),
            },
        )
        .unwrap();
        fs::write(root.join("objects").join("original"), "원본").unwrap();
        save(&root, temp.path(), &db).unwrap();
        let (tool, _) = storage_tool(temp.path()).unwrap();
        let files =
            run_storage_tool(&tool, &root, &["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
        assert!(files.contains(&format!("receipts/{id}.md")));
        assert!(!files.contains("objects/"));
        let message = run_storage_tool(&tool, &root, &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(message.trim(), "저장: 작업 기록");
        let path = root.join("receipts").join(format!("{id}.md"));
        let text = fs::read_to_string(&path).unwrap();
        fs::write(&path, text.replace("수정", "다르게 수정")).unwrap();
        assert!(save(&root, temp.path(), &db).is_err());
    }

    #[test]
    fn check_detects_missing_objects_and_typed_edge_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let text = format!("---\nid: zet_20261115_143052\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets:\n  - sha256:{}\nedges:\n  - type: summarizes\n    target: zet_20261115_143055\n---\n본문\n", "a".repeat(64));
        fs::write(root.join("zettels/zet_20261115_143052.md"), text).unwrap();
        let issues = check(&root, temp.path()).unwrap();
        assert!(issues.iter().any(|i| i.message.contains("원본")));
        assert!(issues.iter().any(|i| i.message.contains("연결")));
    }

    #[test]
    fn check_rejects_wrong_edge_kind_and_extra_head_field() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let text = "---\nid: zet_20261115_143052\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-15T14:30:52+09:00'\nupdated_at: '2026-11-15T14:30:52+09:00'\nassets: []\nedges:\n  - type: invalid\n    target: zet_20261115_143055\n---\n본문\n";
        let path = root.join("zettels/zet_20261115_143052.md");
        fs::write(&path, text).unwrap();
        assert!(!check(&root, temp.path()).unwrap().is_empty());
        fs::write(
            &path,
            text.replace("type: invalid", "type: references")
                .replace("assets: []", "extra: value\nassets: []"),
        )
        .unwrap();
        assert!(!check(&root, temp.path()).unwrap().is_empty());
    }

    #[test]
    fn reindex_recreates_zets_edges_and_receipt_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        initialize(&root, temp.path()).unwrap();
        let db = temp.path().join("momso.db");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let first = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "원본 글".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "본문".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let second = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "요약 글".into(),
                summary: "요약".into(),
                kind: "summary".into(),
                body: "본문".into(),
                assets: vec![],
                edges: vec![crate::archive_zet::Edge {
                    kind: "summarizes".into(),
                    target: first.clone(),
                }],
            },
        )
        .unwrap();
        let receipt = crate::archive_receipt::create(
            &root,
            &db,
            crate::archive_receipt::ReceiptCreate {
                tool: "person".into(),
                model: "".into(),
                chat: None,
                read: vec![first.clone()],
                wrote: vec![second.clone()],
                prompt: "".into(),
                changes: "고침".into(),
            },
        )
        .unwrap();
        save(&root, temp.path(), &db).unwrap();
        let first_history = history(&root, temp.path(), &first).unwrap();
        let second_history = history(&root, temp.path(), &second).unwrap();
        assert_eq!(first_history[0].title.as_deref(), Some("원본 글"));
        assert_eq!(second_history[0].title.as_deref(), Some("요약 글"));
        assert_eq!(first_history[0].other_zets, 1);
        let conn = crate::storage::open_writable_connection(&db).unwrap();
        let counts = || -> (i64, i64, i64) {
            (
                conn.query_row("SELECT count(*) FROM archive_zets", [], |r| r.get(0))
                    .unwrap(),
                conn.query_row("SELECT count(*) FROM archive_zets_edges", [], |r| r.get(0))
                    .unwrap(),
                conn.query_row("SELECT count(*) FROM archive_receipts_wrote", [], |r| {
                    r.get(0)
                })
                .unwrap(),
            )
        };
        let expected = counts();
        conn.execute_batch("DELETE FROM archive_zets; DELETE FROM archive_zets_edges; DELETE FROM archive_receipts_wrote;").unwrap();
        reindex(&root, &db).unwrap();
        assert_eq!(counts(), expected);
        let linked = crate::archive_zet::linked(&root, &db, &second).unwrap();
        let encoded = serde_json::to_value(linked).unwrap();
        assert_eq!(encoded["receipts"][0]["id"], receipt);
        assert_eq!(encoded["receipts"][0]["tool"], "person");
        assert_eq!(encoded["outgoing"][0]["title"], "원본 글");
        assert_eq!(encoded["outgoing"][0]["type"], "summarizes");
        conn.execute("DELETE FROM archive_zets WHERE id=?1", [&first])
            .unwrap();
        let missing =
            serde_json::to_value(crate::archive_zet::linked(&root, &db, &second).unwrap()).unwrap();
        assert_eq!(missing["outgoing"][0]["missing"], true);
        assert!(missing["outgoing"][0]["title"].is_null());
    }
}
