use serde_json::{json, Value};

#[path = "class_student_core.rs"]
mod core;
pub(crate) use core::valid;

pub(crate) static STORE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn dispatch(
    data_root: &std::path::Path,
    archive_root: &std::path::Path,
    method: &str,
    args: &Value,
) -> Value {
    let Ok(_guard) = STORE_LOCK.lock() else {
        return json!({"status":"unavailable"});
    };
    core::dispatch(data_root, archive_root, method, args)
}

pub(crate) async fn dispatch_approved(
    paths: (std::path::PathBuf, std::path::PathBuf),
    chat: &str,
    provider: &str,
    method: String,
    args: Value,
    ask: crate::browser_runtime::Ask,
    cancel: tokio_util::sync::CancellationToken,
) -> Value {
    if !valid(&method, &args) {
        return json!({"status":"invalid_arguments"});
    }
    if matches!(method.as_str(), "momso_crm_upsert" | "momso_crm_mark_read") {
        let count = args.get("records").and_then(Value::as_array).map(Vec::len);
        let request = crate::approval::class_student_request(chat, provider, count);
        if !ask(request, cancel.clone()).await || cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
    }
    tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return json!({"status":"denied"});
        }
        dispatch(&paths.0, &paths.1, &method, &args)
    })
    .await
    .unwrap_or_else(|_| json!({"status":"unavailable"}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let archive = dir.path().join("archive");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        (dir, data, archive)
    }

    fn student(service: &str, id: &str, time: &str, name: &str) -> Value {
        json!({"kind":"student","source":{"service":service,"externalId":id,"observedAt":time},"data":{"name":name,"contact":null,"passes":[{"name":"fictional pass","remaining":4,"startsAt":null,"endsAt":null}]}})
    }

    #[tokio::test]
    async fn writes_require_approval_but_reads_do_not() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let (_dir, data, archive) = fixture();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let deny: crate::browser_runtime::Ask = Arc::new(move |request, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            let serialized = serde_json::to_string(&request).unwrap();
            assert!(!serialized.contains("Fictional A"));
            Box::pin(async { false })
        });
        let args = json!({"records":[student("service-a", "42", "2026-11-20T09:00:00+09:00", "Fictional A")]});
        let token = tokio_util::sync::CancellationToken::new();
        assert_eq!(
            dispatch_approved(
                (data.clone(), archive.clone()),
                "chat",
                "codex",
                "momso_crm_upsert".into(),
                args.clone(),
                deny.clone(),
                token.clone()
            )
            .await["status"],
            "denied"
        );
        assert_eq!(
            dispatch_approved(
                (data.clone(), archive.clone()),
                "chat",
                "codex",
                "momso_crm_mark_read".into(),
                json!({"service":"service-a","observedAt":"2026-11-20T09:00:00+09:00"}),
                deny.clone(),
                token.clone()
            )
            .await["status"],
            "denied"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let allow: crate::browser_runtime::Ask = Arc::new(|_, _| Box::pin(async { true }));
        assert_eq!(
            dispatch_approved(
                (data.clone(), archive.clone()),
                "chat",
                "codex",
                "momso_crm_upsert".into(),
                args.clone(),
                allow.clone(),
                token.clone()
            )
            .await["status"],
            "ok"
        );
        assert_eq!(
            dispatch_approved(
                (data.clone(), archive.clone()),
                "chat",
                "codex",
                "momso_crm_search".into(),
                json!({"kind":"student"}),
                deny.clone(),
                token.clone()
            )
            .await["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        token.cancel();
        assert_eq!(
            dispatch_approved(
                (data, archive),
                "chat",
                "codex",
                "momso_crm_upsert".into(),
                args,
                allow,
                token
            )
            .await["status"],
            "denied"
        );
    }

    #[test]
    fn batch_records_are_local_and_history_is_readable() {
        let (_dir, data, archive) = fixture();
        let a = student(
            "service-a",
            "42",
            "2026-11-20T09:00:00+09:00",
            "Fictional A",
        );
        let class = json!({"kind":"class","source":{"service":"service-a","externalId":"class-1","observedAt":"2026-11-20T09:00:00+09:00"},"data":{"name":"Fictional lesson","startsAt":null,"endsAt":null,"instructor":null,"capacity":null,"reservationCount":null}});
        let reservation = json!({"kind":"reservation","source":{"service":"service-a","externalId":"booking-1","observedAt":"2026-11-20T09:00:00+09:00"},"data":{"classExternalId":"class-1","studentExternalId":"42","status":null}});
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":[a,class,reservation]})
            )["count"],
            3
        );
        let b = student(
            "service-a",
            "42",
            "2026-11-20T10:00:00+09:00",
            "Fictional B",
        );
        assert_eq!(
            dispatch(&data, &archive, "momso_crm_upsert", &json!({"records":[b]}))["status"],
            "ok"
        );
        let got = dispatch(
            &data,
            &archive,
            "momso_crm_read",
            &json!({"kind":"student","service":"service-a","externalId":"42"}),
        );
        assert_eq!(got["record"]["data"]["name"], "Fictional B");
        assert_eq!(got["history"][0]["data"]["name"], "Fictional A");
        assert!(data.join("class-student-private.sqlite3").is_file());
        assert!(!archive.join("class-student-private.sqlite3").exists());
    }

    #[test]
    fn services_do_not_merge_and_search_paginates() {
        let (_dir, data, archive) = fixture();
        let time = "2026-11-20T09:00:00+09:00";
        let records = vec![
            student("a", "1", time, "Fictional"),
            student("b", "1", time, "Fictional"),
        ];
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":records})
            )["status"],
            "ok"
        );
        let first = dispatch(
            &data,
            &archive,
            "momso_crm_search",
            &json!({"kind":"student","query":"Fictional","limit":1}),
        );
        assert_eq!(first["records"].as_array().unwrap().len(), 1);
        assert_eq!(first["nextCursor"], 1);
        let next = dispatch(
            &data,
            &archive,
            "momso_crm_search",
            &json!({"kind":"student","query":"Fictional","limit":1,"cursor":1}),
        );
        assert_ne!(
            first["records"][0]["source"]["service"],
            next["records"][0]["source"]["service"]
        );
        assert!(next["nextCursor"].is_null());
    }

    #[test]
    fn invalid_or_stale_batches_never_partially_write() {
        let (_dir, data, archive) = fixture();
        let a = student("a", "1", "2026-11-20T09:00:00Z", "Fictional");
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":[a.clone()]})
            )["status"],
            "ok"
        );
        let newer = student("a", "2", "2026-11-20T11:00:00Z", "Another");
        let stale = student("a", "1", "2026-11-20T08:00:00Z", "Old");
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":[newer,stale]})
            )["status"],
            "stale_observation"
        );
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_read",
                &json!({"kind":"student","service":"a","externalId":"2"})
            )["status"],
            "not_found"
        );
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":[a.clone(),a]})
            )["status"],
            "duplicate_key"
        );
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":[{"kind":"student","source":{"service":"a","externalId":"","observedAt":"bad"},"data":{}}]})
            )["status"],
            "invalid_arguments"
        );
    }

    #[test]
    fn last_observation_uses_absolute_time_and_returns_null_for_unknown() {
        let (_dir, data, archive) = fixture();
        let a = student("a", "1", "2026-11-20T09:00:00+09:00", "A");
        let b = student("a", "2", "2026-11-20T01:00:00Z", "B");
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_upsert",
                &json!({"records":[a,b]})
            )["status"],
            "ok"
        );
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_last_read",
                &json!({"service":"a"})
            )["lastReadAt"],
            "2026-11-20T01:00:00Z"
        );
        assert!(dispatch(
            &data,
            &archive,
            "momso_crm_last_read",
            &json!({"service":"unknown"})
        )["lastReadAt"]
            .is_null());
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_mark_read",
                &json!({"service":"empty","observedAt":"2026-11-20T12:00:00Z"})
            )["status"],
            "ok"
        );
        assert_eq!(
            dispatch(
                &data,
                &archive,
                "momso_crm_last_read",
                &json!({"service":"empty"})
            )["lastReadAt"],
            "2026-11-20T12:00:00Z"
        );
    }

    #[test]
    fn refuses_local_database_inside_archive() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("config");
        std::fs::create_dir(&data).unwrap();
        assert_eq!(
            dispatch(
                &data,
                dir.path(),
                "momso_crm_last_read",
                &json!({"service":"a"})
            )["status"],
            "unsafe_location"
        );
        assert!(!data.join("class-student-private.sqlite3").exists());
        for path in [
            "class-student-private.sqlite3",
            "config/class-student-private.sqlite3",
            "config/class-student-private.sqlite3-journal",
            "config/class-student-private.sqlite3-wal",
            "config/class-student-private.sqlite3-shm",
        ] {
            assert!(!crate::archive::additional_file_path(path), "{path}");
        }
    }

    #[test]
    fn history_pages_keep_old_values_and_date_only_passes() {
        let (_dir, data, archive) = fixture();
        for n in 0..13 {
            let record = json!({"kind":"student","source":{"service":"a","externalId":"1","observedAt":format!("2026-11-20T{:02}:00:00Z",n)},"data":{"name":format!("Fictional {n}"),"passes":[{"name":"Pass","remaining":null,"startsAt":"2026-11-20","endsAt":"2027-01-01"}]}});
            assert_eq!(
                dispatch(
                    &data,
                    &archive,
                    "momso_crm_upsert",
                    &json!({"records":[record]})
                )["status"],
                "ok"
            );
        }
        let first = dispatch(
            &data,
            &archive,
            "momso_crm_read",
            &json!({"kind":"student","service":"a","externalId":"1"}),
        );
        assert_eq!(first["history"].as_array().unwrap().len(), 10);
        assert_eq!(first["nextHistoryCursor"], 10);
        let second = dispatch(
            &data,
            &archive,
            "momso_crm_read",
            &json!({"kind":"student","service":"a","externalId":"1","historyCursor":10}),
        );
        assert_eq!(second["history"].as_array().unwrap().len(), 2);
        assert!(second["nextHistoryCursor"].is_null());
    }
}
