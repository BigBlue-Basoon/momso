//! Reproducible local-only capacity measurement; run the ignored test explicitly.
use std::{fs, path::Path, process::Command, time::Instant};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(output.status.success(), "git fixture failed: {:?}", args);
    String::from_utf8(output.stdout).unwrap()
}

fn size(path: &Path) -> u64 {
    fs::read_dir(path)
        .unwrap()
        .map(|item| {
            let item = item.unwrap();
            let meta = item.metadata().unwrap();
            if meta.is_dir() {
                size(&item.path())
            } else {
                meta.len()
            }
        })
        .sum()
}

fn measure_large_search(db_path: &Path, room_count: usize, short: &str, long: &str, time: &str) {
    let mut db = rusqlite::Connection::open(db_path).unwrap();
    let tx = db.transaction().unwrap();
    for n in room_count..5000 {
        if n % 500 == 0 {
            println!("BENCH generating_rooms={n}");
        }
        let id = format!("chat_{n:08x}-0000-4000-8000-{n:012x}");
        tx.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES(?1,?2,?3,?3)",
            rusqlite::params![id, format!("fixture room {n}"), time],
        )
        .unwrap();
        for m in 0..200 {
            tx.execute(
                "INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params![
                    format!("message-{n}-{m}"),
                    id,
                    if m % 2 == 0 { "user" } else { "assistant" },
                    if m % 10 == 0 { long } else { short },
                    time
                ],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();
    if std::env::var_os("MOMSO_SCALE_UPGRADE_ONLY").is_some() {
        let start = Instant::now();
        db.execute_batch("INSERT INTO chat_message_search_map(message_rowid,chat_id) SELECT rowid,chat_id FROM chat_messages; INSERT INTO chat_message_search(chat_message_search) VALUES('rebuild');").unwrap();
        println!(
            "BENCH previous_synchronous_index_ms={}",
            start.elapsed().as_millis()
        );
        db.execute_batch("DROP TRIGGER chat_message_search_delete; DROP TRIGGER chat_message_search_move; DROP TRIGGER chat_message_search_update; DROP TABLE chat_message_search_vocab; DROP TABLE chat_message_search; DROP TABLE chat_message_search_map; DROP TABLE chat_search_index_state; DELETE FROM app_schema_migrations WHERE version IN (89,92); PRAGMA user_version=88;").unwrap();
        drop(db);
        let start = Instant::now();
        crate::storage::initialize(
            db_path,
            &db_path.parent().unwrap().join("upgrade-backups"),
            "fixture",
        )
        .unwrap();
        println!(
            "BENCH deferred_upgrade_start_ms={}",
            start.elapsed().as_millis()
        );
        let db = rusqlite::Connection::open(db_path).unwrap();
        let (ready, count): (bool,i64) = db.query_row("SELECT ready,(SELECT count(*) FROM chat_message_search_map) FROM chat_search_index_state WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert!(!ready);
        assert_eq!(count, 0);
        drop(db);
        let start = Instant::now();
        assert!(!crate::chat_search_index::batch(db_path, 128).unwrap());
        println!(
            "BENCH deferred_first_batch_ms={}",
            start.elapsed().as_millis()
        );
        return;
    }
    if std::env::var_os("MOMSO_SCALE_SEARCH_ONLY").is_some() {
        db.execute_batch("INSERT INTO chat_message_search_map(message_rowid,chat_id) SELECT rowid,chat_id FROM chat_messages; INSERT INTO chat_message_search(chat_message_search) VALUES('rebuild'); CREATE TRIGGER chat_message_search_insert AFTER INSERT ON chat_messages BEGIN INSERT INTO chat_message_search(rowid,text) VALUES(new.rowid,new.text); INSERT INTO chat_message_search_map(message_rowid,chat_id) VALUES(new.rowid,new.chat_id); END;").unwrap();
    }
    drop(db);
    let start = Instant::now();
    let _ = crate::chat::search(db_path, "detailed instruction").unwrap();
    println!("BENCH chat_search_5000_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    let page = crate::chat::search_page(db_path, "detailed instruction", 0).unwrap();
    let elapsed = start.elapsed().as_millis();
    let page_json = serde_json::to_value(page).unwrap();
    assert_eq!(page_json["hasMore"], true);
    assert_eq!(page_json["items"].as_array().unwrap().len(), 80);
    println!("BENCH chat_search_page_5000_ms={elapsed} has_more=true");
    let start = Instant::now();
    let _ = crate::chat::search(db_path, "no_such_fixture_needle").unwrap();
    println!(
        "BENCH chat_search_missing_5000_ms={}",
        start.elapsed().as_millis()
    );
}

#[test]
#[ignore = "large synthetic capacity benchmark; run explicitly with --ignored --nocapture"]
fn synthetic_year_capacity() {
    let smoke = std::env::var_os("MOMSO_SCALE_SMOKE").is_some();
    let room_count = if smoke {
        2
    } else {
        std::env::var("MOMSO_SCALE_ROOMS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|n: &usize| (2..=5000).contains(n))
            .unwrap_or(500)
    };
    let document_count = if smoke { 3 } else { 1000 };
    let asset_count = if smoke { 5 } else { 2000 };
    let term_count = if smoke { 240 } else { 1000 };
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let db_path = root.join("momso.db");
    let archive_root = root.join("momso");
    let started = Instant::now();
    crate::storage::initialize(&db_path, &root.join("backups"), "scale").unwrap();
    let location =
        crate::archive_location::ArchiveLocation::load(db_path.clone(), root, vec![]).unwrap();
    crate::archive::initialize(&archive_root, root).unwrap();
    crate::yoga_dictionary::initialize(&archive_root).unwrap();
    println!("BENCH backend_init_ms={}", started.elapsed().as_millis());
    let paths = crate::runtime::RuntimePaths {
        data_root: root.to_path_buf(),
        database_path: db_path.clone(),
        objects_path: archive_root.join("objects"),
        exports_path: root.join("exports"),
        backups_path: root.join("backups"),
        logs_path: root.join("logs"),
        archive: location,
        archive_lease: None,
    };
    let first = crate::archive_zet::create(
        &archive_root,
        &db_path,
        crate::archive_zet::ZetCreate {
            title: "fixture lesson".into(),
            summary: "synthetic".into(),
            kind: "note".into(),
            body: "synthetic document content".into(),
            assets: vec![],
            edges: vec![],
        },
    )
    .unwrap();
    let template =
        fs::read_to_string(archive_root.join("zettels").join(format!("{first}.md"))).unwrap();
    let base = chrono::Local::now().naive_local() - chrono::Duration::seconds(1000);
    for n in 0..document_count - 1 {
        let id = format!(
            "zet_{}",
            (base + chrono::Duration::seconds(n)).format("%Y%m%d_%H%M%S")
        );
        fs::write(
            archive_root.join("zettels").join(format!("{id}.md")),
            template.replace(&first, &id),
        )
        .unwrap();
    }
    crate::archive::reindex(&archive_root, &db_path).unwrap();
    if std::env::var_os("MOMSO_SCALE_CHECK_ONLY").is_some() {
        let start = Instant::now();
        let issues = crate::archive::check(&archive_root, root).unwrap();
        println!(
            "BENCH archive_check_1000_ms={} issues={}",
            start.elapsed().as_millis(),
            issues.len()
        );
        assert!(issues.is_empty());
        return;
    }
    let dictionary = archive_root.join("dictionary/yoga.json");
    let mut data: serde_json::Value =
        serde_json::from_slice(&fs::read(&dictionary).unwrap()).unwrap();
    let terms = data["terms"].as_array_mut().unwrap();
    let term = terms[0].clone();
    while terms.len() < term_count {
        let mut t = term.clone();
        let n = terms.len();
        t["id"] = format!("scale-{n}").into();
        t["sanskrit"] = format!("fixture {n}").into();
        t["roman"] = format!("fixture {n}").into();
        t["korean"] = serde_json::json!([format!("fixture {n}")]);
        t["koreanNames"] = serde_json::json!([format!("fixture {n}")]);
        t["variants"] = serde_json::json!([]);
        terms.push(t);
    }
    fs::write(dictionary, serde_json::to_vec(&data).unwrap()).unwrap();
    let mut db = rusqlite::Connection::open(&db_path).unwrap();
    if std::env::var_os("MOMSO_SCALE_SEARCH_ONLY").is_some() {
        db.execute_batch("DROP TRIGGER chat_message_search_insert;")
            .unwrap();
    }
    let tx = db.transaction().unwrap();
    let time = "2026-01-01T00:00:00+00:00";
    for n in 0..asset_count {
        let hash = format!("{n:064x}");
        tx.execute("INSERT INTO momso_source_assets(id,asset_kind,content_sha256,byte_length,storage_relative_path,original_file_name,captured_by,created_at) VALUES(?1,'raw_transcript',?2,1,?3,?4,'fixture',?5)", rusqlite::params![format!("asset-{n}"),hash,format!("sha256/{hash}"),format!("lesson-{n}.txt"),time]).unwrap();
    }
    let short = "synthetic lesson context ".repeat(12);
    let long = "synthetic detailed instruction and explanation ".repeat(90);
    for n in 0..room_count {
        let id = format!("chat_{n:08x}-0000-4000-8000-{n:012x}");
        tx.execute(
            "INSERT INTO chats(id,title,created_at,updated_at) VALUES(?1,?2,?3,?3)",
            rusqlite::params![id, format!("fixture room {n}"), time],
        )
        .unwrap();
        for m in 0..if n == 0 { 1000 } else { 200 } {
            tx.execute(
                "INSERT INTO chat_messages(id,chat_id,role,text,created_at) VALUES(?1,?2,?3,?4,?5)",
                rusqlite::params![
                    format!("message-{n}-{m}"),
                    id,
                    if m % 2 == 0 { "user" } else { "assistant" },
                    if m % 10 == 0 { &long } else { &short },
                    time
                ],
            )
            .unwrap();
        }
    }
    tx.commit().unwrap();
    drop(db);
    println!("BENCH dataset rooms={room_count} messages={} documents={document_count} asset_ids={asset_count} terms={term_count}", (room_count - 1) * 200 + 1000);
    if std::env::var_os("MOMSO_SCALE_SEARCH_ONLY").is_some() {
        measure_large_search(&db_path, room_count, &short, &long, time);
        return;
    }
    if std::env::var_os("MOMSO_SCALE_RESTORE_ONLY").is_some() {
        crate::chat_archive_store::write_all(&archive_root, &paths).unwrap();
        git(
            &archive_root,
            &[
                "add",
                "--",
                "zettels/",
                "chats/",
                "skills/",
                ".agents/skills/",
                ".claude/skills/",
                "AGENTS.md",
                "CLAUDE.md",
                "GEMINI.md",
            ],
        );
        git(
            &archive_root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
        let room = "chat_00000001-0000-4000-8000-000000000001";
        crate::chat::append(
            &db_path,
            room,
            crate::chat::Role::User,
            "synthetic timing update",
            None,
            None,
        )
        .unwrap();
        let start = Instant::now();
        crate::chat_archive_store::write(&archive_root, &paths, room).unwrap();
        println!(
            "BENCH changed_snapshot_write_ms={}",
            start.elapsed().as_millis()
        );
        let relative = format!("chats/{room}.json");
        let start = Instant::now();
        git(&archive_root, &["add", "--", &relative]);
        println!("BENCH changed_git_add_ms={}", start.elapsed().as_millis());
        let start = Instant::now();
        git(
            &archive_root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--only",
                "-m",
                "fixture update",
                "--",
                &relative,
            ],
        );
        println!(
            "BENCH changed_git_commit_ms={}",
            start.elapsed().as_millis()
        );
        let start = Instant::now();
        crate::storage::initialize(&db_path, &root.join("backups"), "scale").unwrap();
        crate::archive::initialize(&archive_root, root).unwrap();
        println!(
            "BENCH populated_history_backend_reopen_ms={}",
            start.elapsed().as_millis()
        );
        let other = root.join("new.sqlite");
        crate::storage::initialize(&other, &root.join("new-backups"), "scale").unwrap();
        let start = Instant::now();
        crate::chat_archive_store::restore(&archive_root, &other).unwrap();
        println!("BENCH restore_500_ms={}", start.elapsed().as_millis());
        return;
    }
    let check_started = Instant::now();
    let issues = crate::archive::check(&archive_root, root).unwrap();
    println!(
        "BENCH archive_check_1000_ms={} issues={}",
        check_started.elapsed().as_millis(),
        issues.len()
    );
    assert!(
        issues.is_empty(),
        "fixture archive issues: {}",
        serde_json::to_string(&issues).unwrap()
    );
    let start = Instant::now();
    crate::storage::initialize(&db_path, &root.join("backups"), "scale").unwrap();
    println!(
        "BENCH populated_storage_initialize_ms={}",
        start.elapsed().as_millis()
    );
    let archive_start = Instant::now();
    crate::archive::initialize(&archive_root, root).unwrap();
    println!(
        "BENCH populated_archive_initialize_ms={}",
        archive_start.elapsed().as_millis()
    );
    println!(
        "BENCH populated_backend_reopen_ms={}",
        start.elapsed().as_millis()
    );
    for (label, id) in [
        ("write_200_ms", "chat_00000001-0000-4000-8000-000000000001"),
        ("write_1000_ms", "chat_00000000-0000-4000-8000-000000000000"),
    ] {
        let start = Instant::now();
        crate::chat_archive_store::write(&archive_root, &paths, id).unwrap();
        println!(
            "BENCH {label}={} bytes={}",
            start.elapsed().as_millis(),
            fs::metadata(archive_root.join("chats").join(format!("{id}.json")))
                .unwrap()
                .len()
        );
    }
    let start = Instant::now();
    crate::chat_archive_store::write_all(&archive_root, &paths).unwrap();
    println!("BENCH write_all_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    crate::archive::save(&archive_root, root, &db_path).unwrap();
    println!("BENCH initial_git_save_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    crate::chat_archive_store::validate(&archive_root, &db_path).unwrap();
    println!(
        "BENCH snapshot_validate_all_ms={}",
        start.elapsed().as_millis()
    );
    let start = Instant::now();
    git(
        &archive_root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--",
            "chats/",
        ],
    );
    println!("BENCH chat_git_status_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    let issues = crate::archive::check(&archive_root, root).unwrap();
    println!(
        "BENCH full_document_check_ms={} issues={}",
        start.elapsed().as_millis(),
        issues.len()
    );
    let start = Instant::now();
    crate::archive::save_one(
        &archive_root,
        root,
        &db_path,
        &first,
        crate::archive_zet::ZetUpdate {
            title: None,
            summary: None,
            body: Some("synthetic updated document".into()),
            status: None,
            assets: None,
            edges: None,
        },
        None,
    )
    .unwrap();
    println!("BENCH one_document_save_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    crate::chat_archive_store::save_pending(&archive_root, root, &paths).unwrap();
    println!(
        "BENCH establish_tracking_ms={}",
        start.elapsed().as_millis()
    );
    let room = "chat_00000001-0000-4000-8000-000000000001";
    crate::chat::append(
        &db_path,
        room,
        crate::chat::Role::User,
        "synthetic extra",
        None,
        None,
    )
    .unwrap();
    let start = Instant::now();
    let pending = crate::chat_archive_tracking::changes(&db_path).unwrap();
    println!(
        "BENCH changed_lookup_ms={} count={}",
        start.elapsed().as_millis(),
        pending.len()
    );
    let start = Instant::now();
    let _ = crate::chat_archive_snapshot::snapshot(&db_path, room).unwrap();
    println!(
        "BENCH changed_redaction_snapshot_ms={}",
        start.elapsed().as_millis()
    );
    let start = Instant::now();
    crate::chat_archive_store::write(&archive_root, &paths, room).unwrap();
    println!(
        "BENCH changed_snapshot_write_ms={}",
        start.elapsed().as_millis()
    );
    let start = Instant::now();
    crate::chat_archive_store::save_pending(&archive_root, root, &paths).unwrap();
    println!(
        "BENCH one_changed_save_pending_ms={}",
        start.elapsed().as_millis()
    );
    if std::env::var_os("MOMSO_SCALE_SAVE_ONLY").is_some() {
        return;
    }
    let start = Instant::now();
    let rooms = crate::chat::list(&db_path).unwrap();
    println!(
        "BENCH chat_list_ms={} count={}",
        start.elapsed().as_millis(),
        rooms.len()
    );
    let start = Instant::now();
    let _ = crate::chat::read(&db_path, room).unwrap();
    println!("BENCH chat_read_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    let _ = crate::chat::search(&db_path, "detailed instruction").unwrap();
    println!("BENCH chat_search_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    let missing = crate::chat::search(&db_path, "no_such_fixture_needle").unwrap();
    println!(
        "BENCH chat_search_missing_ms={} count={}",
        start.elapsed().as_millis(),
        missing.len()
    );
    let start = Instant::now();
    let documents = crate::archive_zet::list(&db_path, None, None).unwrap();
    println!("BENCH document_list_ms={}", start.elapsed().as_millis());
    let start = Instant::now();
    let documents = serde_json::to_value(documents).unwrap();
    let matches = documents
        .as_array()
        .unwrap()
        .iter()
        .filter(|document| document["title"].as_str().unwrap_or("").contains("lesson"))
        .count();
    println!(
        "BENCH document_filter_proxy_ms={} count={matches}",
        start.elapsed().as_millis()
    );
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let start = Instant::now();
    let _ = crate::source_search::search(&conn, "lesson-1999").unwrap();
    println!("BENCH asset_search_ms={}", start.elapsed().as_millis());
    drop(conn);
    let start = Instant::now();
    let _ = crate::yoga_dictionary::list(&archive_root).unwrap();
    println!("BENCH terms_list_ms={}", start.elapsed().as_millis());
    let other = root.join("new.sqlite");
    crate::storage::initialize(&other, &root.join("new-backups"), "scale").unwrap();
    let start = Instant::now();
    crate::chat_archive_store::restore(&archive_root, &other).unwrap();
    println!("BENCH restore_500_ms={}", start.elapsed().as_millis());
    git(&archive_root, &["gc", "--prune=now"]);
    let before = size(&archive_root.join(".git/objects"));
    let before_total = size(&archive_root.join(".git"));
    for n in 0..10 {
        crate::chat::append(
            &db_path,
            room,
            crate::chat::Role::User,
            &format!("year sample {n}"),
            None,
            None,
        )
        .unwrap();
        crate::chat_archive_store::write(&archive_root, &paths, room).unwrap();
        git(&archive_root, &["add", "--", "chats/"]);
        git(
            &archive_root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "fixture",
            ],
        );
    }
    let loose = size(&archive_root.join(".git/objects"));
    let loose_total = size(&archive_root.join(".git"));
    git(&archive_root, &["gc", "--prune=now"]);
    let after = size(&archive_root.join(".git/objects"));
    let after_total = size(&archive_root.join(".git"));
    println!(
        "BENCH git_objects_before_bytes={before} after_10_bytes={after} extrapolated_365_bytes={} loose_10_bytes={loose} extrapolated_365_loose_bytes={}",
        before + after.saturating_sub(before) * 365 / 10,
        before + loose.saturating_sub(before) * 365 / 10
    );
    println!("BENCH git_total_before_bytes={before_total} after_10_bytes={after_total} extrapolated_365_bytes={} loose_10_bytes={loose_total} extrapolated_365_loose_bytes={}", before_total + after_total.saturating_sub(before_total) * 365 / 10, before_total + loose_total.saturating_sub(before_total) * 365 / 10);
    if !smoke && std::env::var_os("MOMSO_SCALE_NO_LARGE").is_none() {
        measure_large_search(&db_path, room_count, &short, &long, time);
    }
}
