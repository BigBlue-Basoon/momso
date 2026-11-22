include!("chat_export_import_shared.rs");

pub(crate) fn import_with_id(
    db: &Path,
    file: &Path,
    provider: &str,
    id: &str,
    progress: &dyn Fn(u64),
) -> Result<ResultCounts, String> {
    let (_guard, token) = reserve(id)?;
    import_controlled(db, file, provider, &token, progress)
}

#[cfg(test)]
mod tests {
    #[test]
    fn masked_import_comparison_rejects_changed_context_or_ambiguous_markers() {
        let masked = "앞 [비밀값으로 보여 가림: 저장된 값] 뒤";
        assert!(super::matches_redacted_message(
            masked,
            "앞 fake-example 뒤"
        ));
        assert!(!super::matches_redacted_message(
            masked,
            "변경 fake-example 뒤"
        ));
        assert!(!super::matches_redacted_message(
            "[비밀값으로 보여 가림: API 키] [비밀값으로 보여 가림: 토큰]",
            "one two"
        ));
    }
    use super::*;
    use std::io::Write;

    #[test]
    fn import_storage_errors_distinguish_actionable_failures() {
        let error = |code| rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None);
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_BUSY)),
            "storage_busy"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_READONLY)),
            "storage_permission_denied"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_FULL)),
            "storage_full"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_CORRUPT)),
            "storage_corrupt"
        );
        assert_eq!(
            import_storage_error(error(rusqlite::ffi::SQLITE_ERROR)),
            "storage_error"
        );
    }

    #[test]
    fn import_and_preview_keep_database_failures_distinct_from_bad_json() {
        let mut db = Connection::open_in_memory().unwrap();
        let data = br#"[{"uuid":"test","chat_messages":[{"sender":"human","text":"hello"}]}]"#;
        let tx = db.transaction().unwrap();
        let mut counts = ResultCounts::default();
        let mut processed = 0;
        assert_eq!(
            parse(
                &data[..],
                &tx,
                "claude",
                &mut counts,
                &AtomicBool::new(false),
                &|_| {},
                &mut processed
            ),
            Err("storage_error".into())
        );
        drop(tx);
        assert_eq!(
            preview_parse(&data[..], &db, "claude", &mut Preview::default()),
            Err("storage_error".into())
        );
    }

    #[test]
    fn import_file_errors_distinguish_missing_and_permission() {
        use std::io::{Error, ErrorKind};
        assert_eq!(
            file_open_error(Error::from(ErrorKind::NotFound)),
            "file_not_found"
        );
        assert_eq!(
            file_open_error(Error::from(ErrorKind::PermissionDenied)),
            "file_permission_denied"
        );
        assert_eq!(file_open_error(Error::from(ErrorKind::Other)), "file_error");
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("momso.db");
        crate::storage::initialize(&db, &dir.path().join("backup"), "test").unwrap();
        (dir, db)
    }

    #[test]
    fn gemini_json_preview_import_repeat_and_cancellation() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let item = |prompt: &str, time: &str| {
            serde_json::json!({
                "header":"Gemini Apps", "products":["Gemini"], "title":format!("Prompted {prompt}"),
                "time":time, "safeHtmlItem":[{"html":"<p>Answer &amp; details</p>"}], "unknown":true
            })
        };
        std::fs::write(
            &file,
            serde_json::json!([
                item("First question", "2026-01-01T10:00:00Z"),
                item("Second question", "2026-01-01T10:25:00Z"),
                item("Later question", "2026-01-01T11:00:00Z")
            ])
            .to_string(),
        )
        .unwrap();
        let p = preview(&db, &file, "gemini").unwrap();
        assert_eq!(p.conversations, 2);
        let cancelled = AtomicBool::new(false);
        assert!(
            matches!(import_controlled(&db, &file, "gemini", &cancelled, &|_| cancelled.store(true, Ordering::SeqCst)), Err(e) if e == "cancelled")
        );
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 2);
        assert_eq!(import(&db, &file, "gemini").unwrap().duplicate, 3);
        assert_eq!(preview(&db, &file, "gemini").unwrap().already_imported, 2);
        let connection = Connection::open(&db).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM chat_messages WHERE text='Answer & details'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
        drop(connection);
        let root = dir.path();
        let archive_root = root.join("momso");
        let archive =
            crate::archive_location::ArchiveLocation::load(db.clone(), root, vec![]).unwrap();
        crate::archive::initialize(&archive_root, root).unwrap();
        let paths = crate::runtime::RuntimePaths {
            data_root: root.to_path_buf(),
            database_path: db.clone(),
            objects_path: archive_root.join("objects"),
            exports_path: root.join("exports"),
            backups_path: root.join("backup"),
            logs_path: root.join("logs"),
            archive,
            archive_lease: None,
        };
        crate::chat_archive_store::write_all(&archive_root, &paths).unwrap();
        assert_eq!(
            std::fs::read_dir(archive_root.join("chats"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn gemini_details_export_imports_request_response_and_time() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        std::fs::write(&file, serde_json::json!([{
            "header":"Gemini", "title":"Used Gemini Apps",
            "titleUrl":"https://gemini.google.com/app/c/example",
            "time":"2024-02-17T22:05:10.123Z", "products":["Gemini Apps"],
            "details":[{"name":"Request","value":"A question"},{"name":"Response","value":"An answer"}]
        }]).to_string()).unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        let c = Connection::open(&db).unwrap();
        let mut q = c
            .prepare("SELECT role,text,created_at FROM chat_messages ORDER BY rowid")
            .unwrap();
        let messages: Vec<(String, String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!((&*messages[0].0, &*messages[0].1), ("user", "A question"));
        assert_eq!(
            (&*messages[1].0, &*messages[1].1),
            ("assistant", "An answer")
        );
        assert!(messages
            .iter()
            .all(|m| m.2.starts_with("2024-02-17T22:05:10.123")));
    }

    #[test]
    fn gemini_rejects_empty_broken_and_other_google_activity() {
        for bytes in [
            &b"[]"[..],
            &b"{"[..],
            &b"[{\"header\":\"Search\",\"title\":\"query\",\"time\":\"2026-01-01T00:00:00Z\"}]"[..],
        ] {
            assert!(matches!(gemini_parse(bytes), Err(e) if e == "unsupported_format"));
        }
    }

    #[test]
    fn gemini_reexport_with_earlier_and_later_turns_has_no_duplicates() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let turn = |n: i64| serde_json::json!({"header":"Gemini Apps", "title":format!("Prompted question {n}"), "time":DateTime::from_timestamp(1_800_000_000 + n * 60, 0).unwrap().to_rfc3339(), "safeHtmlItem":[{"html":format!("<p>answer {n}</p>")}]});
        std::fs::write(
            &file,
            serde_json::json!((5..15).map(turn).collect::<Vec<_>>()).to_string(),
        )
        .unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        std::fs::write(
            &file,
            serde_json::json!((0..18).map(turn).collect::<Vec<_>>()).to_string(),
        )
        .unwrap();
        let preview = preview(&db, &file, "gemini").unwrap();
        assert_eq!(preview.gemini_new_items, Some(8));
        assert_eq!(preview.gemini_duplicate_items, Some(10));
        let counts = import(&db, &file, "gemini").unwrap();
        assert_eq!(
            (counts.imported, counts.duplicate, counts.conflicts),
            (2, 10, 0)
        );
        let db = Connection::open(&db).unwrap();
        let (count, unique): (i64, i64) = db
            .query_row(
                "SELECT COUNT(*),COUNT(DISTINCT text) FROM chat_messages WHERE role='user'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((count, unique), (18, 18));
    }

    #[test]
    fn gemini_same_timestamp_and_changed_answers_are_conflicts() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let turn = |prompt: &str, answer: &str| serde_json::json!({"header":"Gemini Apps", "title":format!("Prompted {prompt}"), "time":"2026-01-01T10:00:00Z", "safeHtmlItem":[{"html":format!("<p>{answer}</p>")}]});
        std::fs::write(
            &file,
            serde_json::json!([turn("first", "original")]).to_string(),
        )
        .unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        std::fs::write(
            &file,
            serde_json::json!([turn("first", "changed"), turn("other", "answer")]).to_string(),
        )
        .unwrap();
        let counts = import(&db, &file, "gemini").unwrap();
        assert_eq!(
            (counts.imported, counts.duplicate, counts.conflicts),
            (0, 0, 2)
        );
        let db = Connection::open(&db).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn gemini_redacted_match_requires_unique_timestamp() {
        let (dir, db) = fixture();
        let file = dir.path().join("MyActivity.json");
        let turn = |prompt: &str| serde_json::json!({"header":"Gemini Apps", "title":format!("Prompted {prompt}"), "time":"2026-01-01T10:00:00Z"});
        std::fs::write(&file, serde_json::json!([turn("common first")]).to_string()).unwrap();
        assert_eq!(import(&db, &file, "gemini").unwrap().imported, 1);
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "UPDATE chat_messages SET text=?1 WHERE role='user'",
                ["common [비밀값으로 보여 가림: 저장된 값]"],
            )
            .unwrap();
        connection.execute("UPDATE chat_import_message_state SET fingerprint='redacted-v1' WHERE message_id IN (SELECT id FROM chat_messages WHERE role='user')", []).unwrap();
        drop(connection);
        assert_eq!(
            preview(&db, &file, "gemini")
                .unwrap()
                .gemini_duplicate_items,
            Some(1)
        );
        std::fs::write(
            &file,
            serde_json::json!([turn("common first"), turn("common second")]).to_string(),
        )
        .unwrap();
        let counts = import(&db, &file, "gemini").unwrap();
        assert_eq!(
            (counts.imported, counts.duplicate, counts.conflicts),
            (0, 0, 2)
        );
    }

    #[test]
    fn gemini_html_and_size_have_specific_reasons() {
        use std::io::Write;
        let (dir, db) = fixture();
        let html = dir.path().join("MyActivity.html");
        std::fs::write(&html, "<html lang='ko'><div class='outer-cell'><div class='content-cell'>알 수 없는 활동<br>2026. 1. 1. 오후 1:00:00</div></div></html>").unwrap();
        let result = preview(&db, &html, "gemini").unwrap();
        assert_eq!(result.guidance, Some(GEMINI_JSON_GUIDANCE));
        assert_eq!(result.conversations, 0);
        let result = import(&db, &html, "gemini").unwrap();
        assert_eq!(result.guidance, Some(GEMINI_JSON_GUIDANCE));
        assert_eq!(result.imported, 0);
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT COUNT(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let zip_path = dir.path().join("activity.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        zip.start_file(
            "Takeout/My Activity/Gemini Apps/MyActivity.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        write!(zip, "<html></html>").unwrap();
        zip.finish().unwrap();
        assert_eq!(
            preview(&db, &zip_path, "gemini").unwrap().guidance,
            Some(GEMINI_JSON_GUIDANCE)
        );
        assert_eq!(
            import(&db, &zip_path, "gemini").unwrap().guidance,
            Some(GEMINI_JSON_GUIDANCE)
        );
        assert!(
            matches!(gemini_parse(std::io::repeat(b'x').take(GEMINI_MAX_BYTES+1)), Err(e) if e == "gemini_file_too_large")
        );
    }

    #[test]
    fn gemini_html_import_is_deduplicated_and_reports_grouping() {
        use std::io::Write;
        let (dir, db) = fixture();
        let html = "<div class='outer-cell'><div class='content-cell'>Prompted\u{a0}How are you?<br>Mar 10, 2026, 10:25:03 PM PDT<br><p>I am well.</p></div><div class='content-cell'>meta</div></div>";
        let file = dir.path().join("MyActivity.html");
        std::fs::write(&file, html).unwrap();
        let preview = serde_json::to_value(preview(&db, &file, "gemini").unwrap()).unwrap();
        assert_eq!(preview["groupingNotice"], GEMINI_GROUPING_NOTICE);
        let first = import(&db, &file, "gemini").unwrap();
        assert_eq!(first.imported, 1);
        assert_eq!(first.grouping_notice, Some(GEMINI_GROUPING_NOTICE));
        let second = import(&db, &file, "gemini").unwrap();
        assert_eq!((second.imported, second.duplicate), (0, 1));
        let c = Connection::open(&db).unwrap();
        let mut q = c
            .prepare("SELECT role,text,created_at FROM chat_messages ORDER BY rowid")
            .unwrap();
        let messages: Vec<(String, String, String)> = q
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!((&*messages[0].0, &*messages[0].1), ("user", "How are you?"));
        assert_eq!(
            (&*messages[1].0, &*messages[1].1),
            ("assistant", "I am well.")
        );
        assert!(messages
            .iter()
            .all(|m| m.2.starts_with("2026-03-11T05:25:03")));
        let path = dir.path().join("activity.zip");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file(
            "Takeout/My Activity/Search/MyActivity.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        write!(
            zip,
            "[{{\"header\":\"Search\",\"title\":\"Searched\",\"time\":\"2026-03-01T00:00:00Z\"}}]"
        )
        .unwrap();
        zip.start_file(
            "Takeout/My Activity/Gemini Apps/MyActivity.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        write!(zip, "{html}").unwrap();
        zip.finish().unwrap();
        assert_eq!(import(&db, &path, "gemini").unwrap().duplicate, 1);
    }

    #[test]
    #[ignore = "set MOMSO_IMPORT_BENCH_FILE and MOMSO_IMPORT_BENCH_PROVIDER for a synthetic large export"]
    fn benchmark_synthetic_large_export() {
        let file = std::env::var_os("MOMSO_IMPORT_BENCH_FILE").expect("file");
        let provider = std::env::var("MOMSO_IMPORT_BENCH_PROVIDER").expect("provider");
        let (_dir, db) = fixture();
        let counts = import(&db, Path::new(&file), &provider).unwrap();
        assert_eq!(counts.imported, 1);
    }

    #[test]
    fn gemini_zip_accepts_localized_folders() {
        use std::io::Write;
        let (dir, db) = fixture();
        for folder in ["My Activity/Gemini Apps", "내 활동/Gemini 앱"] {
            let archive = dir.path().join("gemini.zip");
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
            zip.start_file(
                format!("Takeout/{folder}/MyActivity.json"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            write!(zip, "{}", serde_json::json!([{"header":"Gemini Apps","title":"Prompted example", "time":"2026-01-01T00:00:00Z", "safeHtmlItem":[{"html":"<p>one</p><p>two</p>"}]}])).unwrap();
            zip.finish().unwrap();
            assert_eq!(preview(&db, &archive, "gemini").unwrap().conversations, 1);
            let (groups, _) = gemini_load(&archive).unwrap();
            assert_eq!(groups[0]["chat_messages"][1]["text"], "one\ntwo");
        }
    }

    #[test]
    #[ignore = "set MOMSO_GEMINI_EXPORT to inspect a real export without printing its contents"]
    fn inspect_gemini_export_counts_only() {
        let path = std::env::var_os("MOMSO_GEMINI_EXPORT").expect("MOMSO_GEMINI_EXPORT");
        match gemini_load(Path::new(&path)) {
            Ok((groups, unreadable)) => {
                let items: usize = groups
                    .iter()
                    .filter_map(|group| group.get("chat_messages").and_then(Value::as_array))
                    .map(|messages| {
                        messages
                            .iter()
                            .filter(|message| {
                                message.get("sender").and_then(Value::as_str) == Some("human")
                            })
                            .count()
                    })
                    .sum();
                println!("items={items} groups={} unreadable={unreadable} reason=unrecognized_or_missing_fields", groups.len())
            }
            Err(reason) => println!("groups=0 unreadable=unknown reason={reason}"),
        }
    }

    #[test]
    fn preview_counts_without_writing_and_cancel_rolls_back() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let item = |id: &str, date: &str| serde_json::json!({"uuid":id,"created_at":date,"chat_messages":[{"sender":"human","text":"private contents"}]});
        std::fs::write(
            &file,
            serde_json::json!([item("a", "2020-01-01T00:00:00Z")]).to_string(),
        )
        .unwrap();
        import(&db, &file, "claude").unwrap();
        std::fs::write(&file, serde_json::json!([item("a","2020-01-01T00:00:00Z"),item("b","2024-01-01T00:00:00Z"),{"uuid":"broken"}]).to_string()).unwrap();
        let p = preview(&db, &file, "claude").unwrap();
        assert_eq!(
            (p.conversations, p.already_imported, p.unreadable),
            (3, 1, 1)
        );
        assert!(p.oldest.unwrap().starts_with("2020-01-01"));
        assert!(p.newest.unwrap().starts_with("2024-01-01"));
        let count = || {
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(count(), 1);
        let cancelled = AtomicBool::new(false);
        let result = import_controlled(&db, &file, "claude", &cancelled, &|n| {
            if n == 1 {
                cancelled.store(true, Ordering::SeqCst);
            }
        });
        assert!(matches!(result, Err(e) if e == "cancelled"));
        assert_eq!(count(), 1);
        assert!(matches!(preview(&db, &file, "gemini"), Err(e) if e == "unsupported_format"));
    }

    #[test]
    fn progress_request_cancels_without_saving_and_cleans_registry() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(
            &file,
            serde_json::json!([
                {"uuid":"one","chat_messages":[{"sender":"human","text":"one"}]},
                {"uuid":"two","chat_messages":[{"sender":"human","text":"two"}]}
            ])
            .to_string(),
        )
        .unwrap();
        let id = format!("test-{}", Uuid::new_v4());
        let result = import_with_id(&db, &file, "claude", &id, &|n| {
            if n == 1 {
                cancel(&id);
            }
        });
        assert!(matches!(result, Err(e) if e == "cancelled"));
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row("SELECT count(*) FROM chats", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(!running().lock().unwrap().contains_key(&id));
    }

    #[test]
    fn imports_chatgpt_branch_once_and_preserves_history() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(&file, serde_json::json!([{"id":"source-1","title":"원래 제목","create_time":1700000000,"current_node":"b","mapping":{"a":{"parent":null,"message":{"author":{"role":"user"},"content":{"parts":["  질문\n"]}}},"b":{"parent":"a","message":{"author":{"role":"assistant"},"create_time":1700000001,"metadata":{"model_slug":"gpt-x"},"content":{"parts":["답"]}}},"other":{"parent":"a","message":{"author":{"role":"assistant"},"content":{"parts":["다른 가지"]}}}}}]).to_string()).unwrap();
        let first = import(&db, &file, "chatgpt").unwrap();
        assert_eq!((first.imported, first.duplicate), (1, 0));
        let second = import(&db, &file, "chatgpt").unwrap();
        assert_eq!((second.imported, second.duplicate), (0, 1));
        let list = serde_json::to_value(crate::chat::list(&db).unwrap()).unwrap();
        assert_eq!(list[0]["importedFrom"], "chatgpt");
        let id = list[0]["id"].as_str().unwrap();
        let detail = serde_json::to_value(crate::chat::read(&db, id).unwrap()).unwrap();
        assert_eq!(detail["messages"][0]["text"], "  질문\n");
        assert!(detail["messages"][0]["createdAt"].is_null());
        assert_eq!(detail["messages"][1]["model"], "gpt-x");
        assert_eq!(detail["messages"][1]["provider"], "codex");
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        let connection = Connection::open(&db).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT provider FROM chat_messages WHERE role='assistant'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "codex"
        );
        connection
            .execute(
                "UPDATE chat_messages SET provider='chatgpt' WHERE role='assistant'",
                [],
            )
            .unwrap();
        drop(connection);
        let legacy = serde_json::to_value(crate::chat::read(&db, id).unwrap()).unwrap();
        assert_eq!(legacy["messages"][1]["provider"], "codex");
        assert_eq!(legacy["importedFrom"], "chatgpt");
        crate::chat::set_archived(&db, id, true);
        let archived = serde_json::to_value(crate::chat::archived_list(&db).unwrap()).unwrap();
        assert_eq!(archived[0]["provider"], "codex");
        let snapshot = crate::chat_archive_snapshot::snapshot(&db, id).unwrap();
        assert_eq!(snapshot["messages"][1]["provider"], "codex");
        assert_eq!(snapshot["imports"][0]["source_provider"], "chatgpt");
        let mut old_snapshot = snapshot.clone();
        old_snapshot["messages"][1]["provider"] = serde_json::json!("chatgpt");
        assert!(crate::chat_archive_snapshot::merge_snapshots(&snapshot, &old_snapshot).is_ok());
        assert_eq!(
            Connection::open(&db)
                .unwrap()
                .query_row(
                    "SELECT provider FROM chat_messages WHERE role='assistant'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "chatgpt"
        );
        let resumed = crate::chat_handoff::prepare(&db, id, "codex", false, "계속").unwrap();
        assert!(resumed.prompt.contains("  질문\n") && resumed.prompt.contains("답"));
        assert!(crate::chat_codex::supported("codex"));
        let handoff = crate::chat_handoff::prepare(&db, id, "claude", false, "이어 질문").unwrap();
        assert!(handoff.prompt.contains("  질문\n"));
        assert!(handoff.prompt.contains("답"));
        assert!(!handoff.prompt.contains("다른 가지"));
    }

    #[test]
    fn imported_claude_and_gemini_keep_canonical_provider_and_source() {
        let (dir, db) = fixture();
        let claude = dir.path().join("claude.json");
        std::fs::write(&claude, serde_json::json!([{"uuid":"claude-room","chat_messages":[{"sender":"assistant","text":"answer"}]}]).to_string()).unwrap();
        import(&db, &claude, "claude").unwrap();
        let gemini = dir.path().join("gemini.json");
        std::fs::write(&gemini, serde_json::json!([{"header":"Gemini Apps","title":"Prompted question","time":"2026-01-01T10:00:00Z","safeHtmlItem":[{"html":"<p>answer</p>"}]}]).to_string()).unwrap();
        import(&db, &gemini, "gemini").unwrap();
        for room in serde_json::to_value(crate::chat::list(&db).unwrap())
            .unwrap()
            .as_array()
            .unwrap()
        {
            let source = room["importedFrom"].as_str().unwrap();
            let detail =
                serde_json::to_value(crate::chat::read(&db, room["id"].as_str().unwrap()).unwrap())
                    .unwrap();
            let assistant = detail["messages"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["role"] == "assistant")
                .unwrap();
            assert_eq!(assistant["provider"], source);
            assert!(assistant["model"].is_null());
        }
    }

    // Based on the reproduction in d26dcab7: later exports must add the reply.
    #[test]
    fn audit_reproduces_later_export_messages_skipped_as_duplicate() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let old = serde_json::json!([{"uuid":"same-id","name":"lesson","chat_messages":[{"sender":"human","text":"first"}]}]);
        std::fs::write(&file, old.to_string()).unwrap();
        assert_eq!(import(&db, &file, "claude").unwrap().imported, 1);
        let later = serde_json::json!([{"uuid":"same-id","name":"lesson","chat_messages":[{"sender":"human","text":"first"},{"sender":"assistant","text":"new reply"}]}]);
        std::fs::write(&file, later.to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!(
            (
                result.imported,
                result.updated,
                result.added_messages,
                result.conflicts
            ),
            (0, 1, 1, 0)
        );
        let count: i64 = Connection::open(&db)
            .unwrap()
            .query_row("SELECT count(*) FROM chat_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
        assert!(std::fs::read_to_string(&file)
            .unwrap()
            .contains("new reply"));
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
    }

    #[test]
    fn reimport_preserves_local_messages_and_reports_divergence() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let export = |msgs: Vec<Value>| {
            serde_json::json!([{"uuid":"source","name":"lesson","chat_messages":msgs}]).to_string()
        };
        let first = serde_json::json!({"uuid":"m1","sender":"human","text":"first"});
        std::fs::write(&file, export(vec![first.clone()])).unwrap();
        import(&db, &file, "claude").unwrap();
        let c = Connection::open(&db).unwrap();
        let chat: String = c
            .query_row("SELECT chat_id FROM chat_imports", [], |r| r.get(0))
            .unwrap();
        drop(c);
        crate::chat::append(
            &db,
            &chat,
            crate::chat::Role::User,
            "local reply",
            None,
            None,
        )
        .unwrap();
        let next = serde_json::json!({"uuid":"m2","sender":"assistant","text":"export reply"});
        std::fs::write(&file, export(vec![first.clone(), next.clone()])).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.added_messages, result.conflicts), (1, 0));
        let read = serde_json::to_value(crate::chat::read(&db, &chat).unwrap()).unwrap();
        assert_eq!(read["messages"][1]["text"], "local reply");
        assert_eq!(read["messages"][2]["text"], "export reply");
        let altered = serde_json::json!({"uuid":"m1","sender":"human","text":"changed"});
        std::fs::write(&file, export(vec![altered, next])).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.added_messages, result.conflicts), (0, 1));
        assert_eq!(
            serde_json::to_value(crate::chat::read(&db, &chat).unwrap()).unwrap(),
            read
        );
    }

    #[test]
    fn omitted_attachments_are_counted_as_items_not_messages() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"sender":"human","text":"hello","attachments":[{"file_name":"one"},{"file_name":"two"}]},{"sender":"assistant","content":[{"type":"text","text":"answer"},{"type":"image","source":{}}]}]}]).to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!(result.omitted_non_text_messages, 2);
        assert_eq!(result.omitted_attachments, 3);
    }

    #[test]
    fn attachment_references_in_metadata_and_content_are_not_double_counted() {
        let message = serde_json::json!({"metadata":{"attachments":[{"id":"file-one"}]},"content":{"content_type":"multimodal_text","parts":["hello",{"content_type":"image_asset_pointer","asset_pointer":"file-service://file-one"},{"content_type":"image_asset_pointer","asset_pointer":"file-service://file-two"}]}});
        assert_eq!(omitted_attachments(&message), 2);
        assert_eq!(
            omitted_attachments(
                &serde_json::json!({"content":[{"type":"thinking","thinking":"private"}]})
            ),
            0
        );
        let message = serde_json::json!({"metadata":{"attachments":[{"id":null,"file_id":"file-one"}]},"content":{"parts":[{"asset_pointer":"file-service://file-one"},{"file_id":"file-two"}]}});
        assert_eq!(omitted_attachments(&message), 2);
        assert_eq!(
            omitted_attachments(
                &serde_json::json!({"content":{"type":null,"content_type":"image"}})
            ),
            1
        );
    }

    #[test]
    fn legacy_import_with_ambiguous_local_tail_is_not_overwritten() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('local','legacy','user','local','')", []).unwrap();
        drop(c);
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"sender":"human","text":"first"},{"sender":"assistant","text":"later"}]}]).to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.conflicts, result.added_messages), (1, 0));
        let detail = serde_json::to_value(crate::chat::read(&db, "legacy").unwrap()).unwrap();
        assert_eq!(detail["messages"][1]["text"], "local");
    }

    #[test]
    fn legacy_import_with_matching_history_adds_new_export_messages() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        drop(c);
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"uuid":"m1","sender":"human","text":"first"},{"uuid":"m2","sender":"assistant","text":"later"}]}]).to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!(
            (result.updated, result.added_messages, result.conflicts),
            (1, 1, 0)
        );
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
        let detail = serde_json::to_value(crate::chat::read(&db, "legacy").unwrap()).unwrap();
        assert_eq!(detail["messages"].as_array().unwrap().len(), 2);
        assert_eq!(detail["messages"][1]["text"], "later");
    }

    #[test]
    fn matching_legacy_reimport_tracks_original_before_local_continuation() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        drop(c);
        let original = serde_json::json!([{"uuid":"source","chat_messages":[{"uuid":"m1","sender":"human","text":"first"}]}]);
        std::fs::write(&file, original.to_string()).unwrap();
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
        crate::chat::append(&db, "legacy", crate::chat::Role::User, "local", None, None).unwrap();
        let later = serde_json::json!([{"uuid":"source","chat_messages":[{"uuid":"m1","sender":"human","text":"first"},{"uuid":"m2","sender":"assistant","text":"later"}]}]);
        std::fs::write(&file, later.to_string()).unwrap();
        let result = import(&db, &file, "claude").unwrap();
        assert_eq!((result.added_messages, result.conflicts), (1, 0));
        let detail = serde_json::to_value(crate::chat::read(&db, "legacy").unwrap()).unwrap();
        assert_eq!(detail["messages"][1]["text"], "local");
        assert_eq!(detail["messages"][2]["text"], "later");
    }

    #[test]
    fn shorter_legacy_export_does_not_claim_an_unknown_tail() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES('legacy','title','','')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO chat_imports(chat_id,source_provider,source_id) VALUES('legacy','claude','source')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('one','legacy','user','first','')", []).unwrap();
        c.execute("INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES('two','legacy','assistant','unknown tail','')", []).unwrap();
        drop(c);
        std::fs::write(&file, serde_json::json!([{"uuid":"source","chat_messages":[{"sender":"human","text":"first"}]}]).to_string()).unwrap();
        assert_eq!(import(&db, &file, "claude").unwrap().duplicate, 1);
        let c = Connection::open(&db).unwrap();
        let count: i64 = c
            .query_row(
                "SELECT count(*) FROM chat_import_message_state WHERE chat_id='legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn chatgpt_new_branch_message_and_attachment_status_are_reported() {
        let (dir, db) = fixture();
        let file = dir.path().join("conversations.json");
        let mut source = serde_json::json!([{"id":"source","current_node":"a","mapping":{"a":{"parent":null,"message":{"id":"m1","author":{"role":"user"},"content":{"parts":["hello"]}}}}}]);
        std::fs::write(&file, source.to_string()).unwrap();
        import(&db, &file, "chatgpt").unwrap();
        source[0]["current_node"] = "b".into();
        source[0]["mapping"]["b"] = serde_json::json!({"parent":"a","message":{"id":"m2","author":{"role":"assistant"},"content":{"parts":["reply",{"content_type":"image_asset_pointer","asset_pointer":"file-service://file-one"}]}}});
        std::fs::write(&file, source.to_string()).unwrap();
        let result = serde_json::to_value(import(&db, &file, "chatgpt").unwrap()).unwrap();
        assert_eq!(result["updated"], 1);
        assert_eq!(result["addedMessages"], 1);
        assert_eq!(result["omittedAttachments"], 1);
        source[0]["mapping"]["a"]["message"]["id"] = "different-id".into();
        std::fs::write(&file, source.to_string()).unwrap();
        assert_eq!(import(&db, &file, "chatgpt").unwrap().conflicts, 1);
    }

    #[test]
    fn imports_claude_zip_and_rejects_unknown_gemini() {
        let (dir, db) = fixture();
        let path = dir.path().join("export.zip");
        let file = File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            "conversations.json",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated),
        )
        .unwrap();
        write!(zip, "{}", serde_json::json!([{"uuid":"s-1","name":"이름","chat_messages":[{"sender":"human","text":"안녕"},{"sender":"assistant","content":[{"type":"text","text":"반가워"}],"created_at":"2025-01-01T00:00:00Z"}]}])).unwrap();
        zip.finish().unwrap();
        assert_eq!(import(&db, &path, "claude").unwrap().imported, 1);
        let list = serde_json::to_value(crate::chat::list(&db).unwrap()).unwrap();
        let detail =
            serde_json::to_value(crate::chat::read(&db, list[0]["id"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(detail["messages"][1]["text"], "반가워");
        assert_eq!(detail["messages"][1]["createdAt"], "2025-01-01T00:00:00Z");
        assert!(detail["messages"][1]["model"].is_null());
        assert!(matches!(import(&db,&path,"gemini"),Err(e) if e == "unsupported_format"));
    }

    #[test]
    fn malformed_file_rolls_back_all_conversations() {
        let (dir, db) = fixture();
        let path = dir.path().join("bad.json");
        std::fs::write(
            &path,
            r#"[{"uuid":"one","chat_messages":[{"sender":"human","text":"x"}]}, {"oops": ]"#,
        )
        .unwrap();
        assert!(import(&db, &path, "claude").is_err());
        assert!(crate::chat::list(&db).unwrap().is_empty());
    }

    #[test]
    fn streams_many_conversations_and_deduplicates() {
        let (dir, db) = fixture();
        let path = dir.path().join("many.json");
        let mut f = File::create(&path).unwrap();
        write!(f, "[").unwrap();
        for n in 0..2000 {
            if n != 0 {
                write!(f, ",").unwrap();
            }
            write!(f,"{{\"uuid\":\"{n}\",\"chat_messages\":[{{\"sender\":\"human\",\"text\":\"hello\"}}]}}").unwrap();
        }
        write!(f, "]").unwrap();
        drop(f);
        assert_eq!(import(&db, &path, "claude").unwrap().imported, 2000);
        assert_eq!(import(&db, &path, "claude").unwrap().duplicate, 2000);
    }
}
