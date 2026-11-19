//! Approved document changes through the authenticated per-chat bridge.
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(crate) fn valid(method: &str, args: &Value) -> bool {
    let Some(fields) = args.as_object() else {
        return false;
    };
    let text = |name: &str, max: usize| {
        fields
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= max)
    };
    match method {
        "momso_document_create" => {
            fields
                .keys()
                .all(|key| matches!(key.as_str(), "title" | "summary" | "body" | "folder"))
                && text("title", 600)
                && text("summary", 1200)
                && text("body", 32000)
                && fields.get("folder").is_none_or(|value| {
                    value
                        .as_str()
                        .is_some_and(|folder| crate::archive_zet::validate_folder(folder).is_ok())
                })
        }
        "momso_document_edit" => {
            fields.len() == 5
                && fields
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(crate::archive_zet::valid_zet_id)
                && fields
                    .get("expectedHash")
                    .and_then(Value::as_str)
                    .is_some_and(|v| {
                        v.len() == 71
                            && v.starts_with("sha256:")
                            && v[7..].bytes().all(|b| b.is_ascii_hexdigit())
                    })
                && text("title", 600)
                && text("summary", 1200)
                && text("body", 32000)
        }
        _ => false,
    }
}

pub(crate) async fn dispatch(
    paths: (PathBuf, PathBuf),
    chat: &str,
    provider: &str,
    method: &str,
    args: Value,
    ask: crate::browser_runtime::Ask,
    cancel: CancellationToken,
) -> Value {
    let (root, database) = paths;
    if !valid(method, &args) {
        return json!({"status":"invalid_arguments"});
    }
    let id = args["id"].as_str().map(str::to_owned);
    let title = args["title"].as_str().unwrap().to_owned();
    let summary = args["summary"].as_str().unwrap().to_owned();
    let body = args["body"].as_str().unwrap().to_owned();
    let folder = args
        .get("folder")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let hash = args["expectedHash"].as_str().map(str::to_owned);
    if let Some(id) = &id {
        let Ok((head, old_body)) = crate::momso_document_read::read_current(&root, id) else {
            return json!({"status":"unavailable"});
        };
        let Ok(raw) = crate::archive_zet::render(&head, &old_body) else {
            return json!({"status":"unavailable"});
        };
        if Some(crate::archive_zet::content_hash(&raw)) != hash {
            return json!({"status":"changed"});
        }
    }
    let request = crate::approval::document_request(chat, provider, &title, id.as_deref());
    if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
        return json!({"status":"denied"});
    }
    let outcome = tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        let dir = root.join("zettels");
        let Ok(metadata) = std::fs::symlink_metadata(&dir) else {
            return json!({"status":"unavailable"});
        };
        if !metadata.is_dir() || crate::archive_location::is_link_or_reparse(&metadata) {
            return json!({"status":"unavailable"});
        }
        let _guard = crate::chat_file_changes::lock_app_write();
        let result = if let Some(id) = id {
            if crate::momso_document_read::read_current(&root, &id).is_err() {
                return json!({"status":"unavailable"});
            }
            crate::archive_zet::update_checked(
                &root,
                &database,
                &id,
                crate::archive_zet::ZetUpdate {
                    title: Some(title),
                    summary: Some(summary),
                    body: Some(body),
                    status: None,
                    assets: None,
                    edges: None,
                },
                hash.as_deref(),
            )
            .map(|()| id)
        } else {
            crate::archive_zet::create_in_folder(
                &root,
                &database,
                crate::archive_zet::ZetCreate {
                    title,
                    summary,
                    body,
                    kind: "note".into(),
                    assets: vec![],
                    edges: vec![],
                },
                &folder,
            )
        };
        match result {
            Ok(id) => json!({"status":"ok","id":id}),
            Err(message) if message == "이 글이 그사이 바뀌었습니다" => {
                json!({"status":"changed"})
            }
            Err(_) => json!({"status":"unavailable"}),
        }
    })
    .await;
    outcome.unwrap_or_else(|_| json!({"status":"unavailable"}))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn edit_changed_during_approval_does_not_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        let db = temp.path().join("db.sqlite");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let id = crate::archive_zet::create(
            &root,
            &db,
            crate::archive_zet::ZetCreate {
                title: "원문".into(),
                summary: "요약".into(),
                kind: "note".into(),
                body: "원문".into(),
                assets: vec![],
                edges: vec![],
            },
        )
        .unwrap();
        let path = root.join("zettels").join(format!("{id}.md"));
        let original = std::fs::read_to_string(&path).unwrap();
        let hash = crate::archive_zet::content_hash(&original);
        let newer = format!("{original} 사람이 고침");
        let expected = newer.clone();
        let ask: crate::browser_runtime::Ask = Arc::new(move |_, _| {
            std::fs::write(&path, &newer).unwrap();
            Box::pin(async { true })
        });
        let result = dispatch(
            (root.clone(), db),
            "chat",
            "codex",
            "momso_document_edit",
            json!({"id":id,"expectedHash":hash,"title":"변경","summary":"요약","body":"AI"}),
            ask,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(result["status"], "changed");
        assert_eq!(
            std::fs::read_to_string(root.join("zettels").join(format!("{id}.md"))).unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn marketing_skill_creates_approved_document_in_promotion_folder() {
        let skill = include_str!("../bundled-skills/marketing-preparation/SKILL.md");
        assert!(skill.contains("momso_document_create"));
        assert!(skill.contains("folder: \"홍보\""));
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        let db = temp.path().join("db.sqlite");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        crate::archive_zet::create_folder(&root, &db, "홍보").unwrap();
        assert!(crate::archive_zet::folders(&root)
            .unwrap()
            .contains(&"홍보".to_string()));
        let ask: crate::browser_runtime::Ask = Arc::new(|request, _| {
            assert_eq!(request.kind, "file_change");
            Box::pin(async { true })
        });
        let result = dispatch(
            (root.clone(), db),
            "chat",
            "codex",
            "momso_document_create",
            json!({"title":"홍보 글","summary":"요약","body":"수업 안내","folder":"홍보"}),
            ask,
            CancellationToken::new(),
        )
        .await;
        assert_eq!(result["status"], "ok");
        let (head, body) =
            crate::momso_document_read::read_current(&root, result["id"].as_str().unwrap())
                .unwrap();
        assert_eq!(head.folder, "홍보");
        assert_eq!(body, "수업 안내");
    }

    #[tokio::test]
    async fn approved_create_edit_and_stale_edit_preserve_document() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("archive");
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        let db = temp.path().join("db.sqlite");
        crate::storage::initialize(&db, &temp.path().join("backups"), "test").unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        let allow: crate::browser_runtime::Ask = Arc::new(move |request, _| {
            assert_eq!(request.kind, "file_change");
            seen.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { true })
        });
        let deny: crate::browser_runtime::Ask = Arc::new(|_, _| Box::pin(async { false }));
        let args = json!({"title":"시험","summary":"요약","body":"원문","folder":"수업/기록"});
        assert!(!valid(
            "momso_document_create",
            &json!({"title":"시험","summary":"요약","body":"원문","folder":"../기록"})
        ));
        assert!(valid(
            "momso_document_create",
            &json!({"title":"시험","summary":"요약","body":"원문"})
        ));
        assert_eq!(
            dispatch(
                (root.clone(), db.clone()),
                "chat",
                "codex",
                "momso_document_create",
                args.clone(),
                deny,
                CancellationToken::new()
            )
            .await["status"],
            "denied"
        );
        assert_eq!(std::fs::read_dir(root.join("zettels")).unwrap().count(), 0);
        let created = dispatch(
            (root.clone(), db.clone()),
            "chat",
            "codex",
            "momso_document_create",
            args,
            allow.clone(),
            CancellationToken::new(),
        )
        .await;
        assert_eq!(created["status"], "ok");
        let id = created["id"].as_str().unwrap();
        let (head, old) = crate::momso_document_read::read_current(&root, id).unwrap();
        assert_eq!(head.folder, "수업/기록");
        let hash =
            crate::archive_zet::content_hash(&crate::archive_zet::render(&head, &old).unwrap());
        let edit =
            json!({"id":id,"expectedHash":hash,"title":"수정","summary":"요약","body":"고침"});
        assert_eq!(
            dispatch(
                (root.clone(), db.clone()),
                "chat",
                "codex",
                "momso_document_edit",
                edit.clone(),
                allow.clone(),
                CancellationToken::new()
            )
            .await["status"],
            "ok"
        );
        assert_eq!(
            dispatch(
                (root.clone(), db),
                "chat",
                "codex",
                "momso_document_edit",
                edit,
                allow,
                CancellationToken::new()
            )
            .await["status"],
            "changed"
        );
        assert_eq!(
            crate::momso_document_read::read_current(&root, id)
                .unwrap()
                .1,
            "고침"
        );
        assert_eq!(
            crate::momso_document_read::read_current(&root, id)
                .unwrap()
                .0
                .folder,
            "수업/기록"
        );
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}
