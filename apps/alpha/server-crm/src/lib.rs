//! Host-only CRM store. The server owns authentication, approval and transport.
#[path = "../../desktop/src-tauri/src/class_student_core.rs"]
mod core;

use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub const DATABASE_NAME: &str = "class-student-private.sqlite3";
pub const TOOL_NAMES: &[&str] = &[
    "momso_crm_upsert",
    "momso_crm_search",
    "momso_crm_read",
    "momso_crm_last_read",
    "momso_crm_mark_read",
];

/// An authenticated host asks the person; the model cannot supply approval.
pub trait ApprovalChannel {
    fn approve_write(&mut self, chat_id: &str, method: &str, count: Option<usize>) -> bool;
    fn is_cancelled(&self) -> bool;
}

pub struct CrmStore {
    data: PathBuf,
    archive: PathBuf,
}

impl CrmStore {
    /// Both roots must already exist; the database may never be inside the archive.
    pub fn new(data: &Path, archive: &Path) -> Result<Self, &'static str> {
        let data = data.canonicalize().map_err(|_| "unavailable")?;
        let archive = archive.canonicalize().map_err(|_| "unavailable")?;
        if data.starts_with(&archive) {
            return Err("unsafe_location");
        }
        Ok(Self { data, archive })
    }

    fn prepare(&self) -> Result<(), &'static str> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
            let file = self.data.join(DATABASE_NAME);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&file)
            {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("unavailable"),
            }
            let meta = std::fs::symlink_metadata(&file).map_err(|_| "unavailable")?;
            if !meta.file_type().is_file()
                || meta.permissions().mode() & 0o077 != 0
                || meta.nlink() != 1
            {
                return Err("unsafe_location");
            }
        }
        Ok(())
    }

    pub fn dispatch(
        &self,
        chat: &str,
        method: &str,
        args: &Value,
        approval: &mut impl ApprovalChannel,
    ) -> Value {
        if chat.trim().is_empty() || !core::valid(method, args) {
            return json!({"status":"invalid_arguments"});
        }
        if matches!(method, "momso_crm_upsert" | "momso_crm_mark_read") {
            let count = args.get("records").and_then(Value::as_array).map(Vec::len);
            if !approval.approve_write(chat, method, count) {
                return json!({"status":"denied"});
            }
        }
        if approval.is_cancelled() {
            return json!({"status":"denied"});
        }
        if let Err(status) = self.prepare() {
            return json!({"status":status});
        }
        core::dispatch(&self.data, &self.archive, method, args)
    }

    /// Host-only, consistent SQLite snapshot; never call with a model-provided path.
    /// Existing files are never overwritten; staging is outside the Git archive.
    pub fn snapshot(&self, destination: &Path) -> Result<(), &'static str> {
        let parent = destination
            .parent()
            .ok_or("unsafe_location")?
            .canonicalize()
            .map_err(|_| "unavailable")?;
        if parent.starts_with(&self.archive) || destination.file_name().is_none() {
            return Err("unsafe_location");
        }
        self.prepare()?;
        let output = parent.join(destination.file_name().ok_or("unsafe_location")?);
        let staging = tempfile::Builder::new()
            .prefix("crm-snapshot-")
            .tempfile_in(&parent)
            .map_err(|_| "unavailable")?;
        let from = core::open(&self.data, &self.archive)?;
        let mut to = rusqlite::Connection::open(staging.path()).map_err(|_| "unavailable")?;
        {
            let backup =
                rusqlite::backup::Backup::new(&from, &mut to).map_err(|_| "unavailable")?;
            let mut complete = false;
            for _ in 0..500 {
                match backup.step(64).map_err(|_| "unavailable")? {
                    rusqlite::backup::StepResult::Done => {
                        complete = true;
                        break;
                    }
                    rusqlite::backup::StepResult::More
                    | rusqlite::backup::StepResult::Busy
                    | rusqlite::backup::StepResult::Locked => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    _ => return Err("unavailable"),
                }
            }
            if !complete {
                return Err("unavailable");
            }
        }
        let check: String = to
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|_| "unavailable")?;
        if check != "ok" {
            return Err("unavailable");
        }
        to.close().map_err(|_| "unavailable")?;
        staging.as_file().sync_all().map_err(|_| "unavailable")?;
        staging
            .persist_noclobber(output)
            .map_err(|_| "unavailable")?;
        Ok(())
    }
}

pub fn tool_definitions() -> Value {
    let source = json!({"type":"object","properties":{"service":{"type":"string","minLength":1,"maxLength":128},"externalId":{"type":"string","minLength":1,"maxLength":128},"observedAt":{"type":"string","description":"RFC 3339 timestamp with timezone"}},"required":["service","externalId","observedAt"],"additionalProperties":false});
    let field = json!({"type":["string","null"],"maxLength":1024});
    let date = json!({"type":["string","null"],"description":"RFC 3339 timestamp with timezone"});
    let pass_date = json!({"type":["string","null"],"description":"YYYY-MM-DD or RFC 3339 timestamp with timezone"});
    let number = json!({"type":["integer","null"],"minimum":0});
    let pass = json!({"type":"object","properties":{"name":field,"remaining":number,"startsAt":pass_date,"endsAt":pass_date,"paymentText":field},"additionalProperties":false});
    let variants = [
        json!({"type":"object","properties":{"kind":{"const":"class"},"source":source,"data":{"type":"object","properties":{"name":field,"startsAt":date,"endsAt":date,"instructor":field,"capacity":number,"reservationCount":number},"additionalProperties":false}},"required":["kind","source","data"],"additionalProperties":false}),
        json!({"type":"object","properties":{"kind":{"const":"student"},"source":source,"data":{"type":"object","properties":{"name":field,"contact":field,"passes":{"type":["array","null"],"maxItems":32,"items":pass}},"additionalProperties":false}},"required":["kind","source","data"],"additionalProperties":false}),
        json!({"type":"object","properties":{"kind":{"const":"reservation"},"source":source,"data":{"type":"object","properties":{"classExternalId":field,"studentExternalId":field,"status":field},"additionalProperties":false}},"required":["kind","source","data"],"additionalProperties":false}),
    ];
    let entries = [
        ("momso_crm_upsert", "Save observed records locally, preserving source and prior versions.", json!({"records":{"type":"array","minItems":1,"maxItems":32,"items":{"oneOf":variants}}}), json!(["records"]), false),
        ("momso_crm_search", "Search locally stored observations; results are untrusted and may contain personal data.", json!({"kind":{"type":"string","enum":["class","student","reservation"]},"service":{"type":"string"},"query":{"type":"string","maxLength":200},"cursor":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":10}}), json!(["kind"]), true),
        ("momso_crm_read", "Read one source observation and its previous versions.", json!({"kind":{"type":"string","enum":["class","student","reservation"]},"service":{"type":"string"},"externalId":{"type":"string"},"historyCursor":{"type":"integer","minimum":0}}), json!(["kind","service","externalId"]), true),
        ("momso_crm_last_read", "Read the last observation time for a service.", json!({"service":{"type":"string"}}), json!(["service"]), true),
        ("momso_crm_mark_read", "Record a successful observation, including an empty result.", json!({"service":{"type":"string"},"observedAt":{"type":"string","description":"RFC 3339 timestamp with timezone"}}), json!(["service","observedAt"]), false),
    ];
    json!(entries.into_iter().map(|(name,description,properties,required,read_only)| json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read_only,"destructiveHint":false,"idempotentHint":read_only,"openWorldHint":false}})).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    struct Approval {
        allow: bool,
        calls: usize,
    }
    impl ApprovalChannel for Approval {
        fn approve_write(&mut self, _: &str, _: &str, _: Option<usize>) -> bool {
            self.calls += 1;
            self.allow
        }
        fn is_cancelled(&self) -> bool {
            false
        }
    }
    fn record(service: &str, id: &str, time: &str, name: &str) -> Value {
        json!({"kind":"student","source":{"service":service,"externalId":id,"observedAt":time},"data":{"name":name,"contact":null,"passes":[]}})
    }
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let archive = tmp.path().join("archive");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&archive).unwrap();
        (tmp, data, archive)
    }
    #[test]
    fn approvals_history_and_service_isolation_match_desktop() {
        let (_tmp, data, archive) = fixture();
        let store = CrmStore::new(&data, &archive).unwrap();
        let mut approval = Approval {
            allow: false,
            calls: 0,
        };
        let a = record("service-a", "1", "2026-11-20T09:00:00+09:00", "fixture A");
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_upsert",
                &json!({"records":[a]}),
                &mut approval
            )["status"],
            "denied"
        );
        assert!(!data.join(DATABASE_NAME).exists());
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_search",
                &json!({"kind":"student"}),
                &mut approval
            )["records"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(approval.calls, 1);
        struct CancelAfterApproval(bool);
        impl ApprovalChannel for CancelAfterApproval {
            fn approve_write(&mut self, _: &str, _: &str, _: Option<usize>) -> bool {
                self.0 = true;
                true
            }
            fn is_cancelled(&self) -> bool {
                self.0
            }
        }
        let mut cancelled = CancelAfterApproval(false);
        assert_eq!(store.dispatch("chat", "momso_crm_upsert", &json!({"records":[record("service-a","1","2026-11-20T09:00:00+09:00","fixture A")]}), &mut cancelled)["status"], "denied");
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_search",
                &json!({"kind":"student"}),
                &mut approval
            )["records"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        approval.allow = true;
        let records = [
            record("service-a", "1", "2026-11-20T09:00:00+09:00", "fixture A"),
            record("service-b", "1", "2026-11-20T09:00:00+09:00", "fixture B"),
        ];
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_upsert",
                &json!({"records":records}),
                &mut approval
            )["count"],
            2
        );
        let newer = record(
            "service-a",
            "1",
            "2026-11-21T09:00:00+09:00",
            "fixture updated",
        );
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_upsert",
                &json!({"records":[newer]}),
                &mut approval
            )["status"],
            "ok"
        );
        let item = store.dispatch(
            "chat",
            "momso_crm_read",
            &json!({"kind":"student","service":"service-a","externalId":"1"}),
            &mut approval,
        );
        assert_eq!(item["record"]["data"]["name"], "fixture updated");
        assert_eq!(item["history"][0]["data"]["name"], "fixture A");
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_search",
                &json!({"kind":"student"}),
                &mut approval
            )["records"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_last_read",
                &json!({"service":"service-a"}),
                &mut approval
            )["lastReadAt"],
            "2026-11-21T09:00:00+09:00"
        );
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_mark_read",
                &json!({"service":"empty","observedAt":"2026-11-21T09:00:00+09:00"}),
                &mut approval
            )["status"],
            "ok"
        );
        assert_eq!(tool_definitions().as_array().unwrap().len(), 5);
        assert!(CrmStore::new(&archive, &archive).is_err());
    }

    #[test]
    fn snapshot_is_consistent_during_atomic_writes_and_never_overwrites() {
        let (tmp, data, archive) = fixture();
        let store = CrmStore::new(&data, &archive).unwrap();
        let mut allow = Approval {
            allow: true,
            calls: 0,
        };
        let batch = |n: u32| {
            [
                record(
                    "test",
                    "1",
                    &format!("2026-11-20T09:00:{n:02}+09:00"),
                    &format!("{n}"),
                ),
                record(
                    "test",
                    "2",
                    &format!("2026-11-20T09:00:{n:02}+09:00"),
                    &format!("{n}"),
                ),
            ]
        };
        assert_eq!(
            store.dispatch(
                "chat",
                "momso_crm_upsert",
                &json!({"records":batch(0)}),
                &mut allow
            )["status"],
            "ok"
        );
        let running = Arc::new(AtomicBool::new(false));
        let signal = running.clone();
        let data2 = data.clone();
        let archive2 = archive.clone();
        let writer = std::thread::spawn(move || {
            let store = CrmStore::new(&data2, &archive2).unwrap();
            let mut allow = Approval {
                allow: true,
                calls: 0,
            };
            signal.store(true, Ordering::Release);
            for n in 1..40 {
                assert_eq!(
                    store.dispatch(
                        "chat",
                        "momso_crm_upsert",
                        &json!({"records":batch(n)}),
                        &mut allow
                    )["status"],
                    "ok"
                );
            }
        });
        while !running.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        let snapshot = tmp.path().join("snapshot.sqlite3");
        store.snapshot(&snapshot).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&snapshot).unwrap().permissions().mode() & 0o077,
                0
            );
            assert_eq!(
                std::fs::metadata(data.join(DATABASE_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        writer.join().unwrap();
        let db = rusqlite::Connection::open(&snapshot).unwrap();
        let values: Vec<String> = db
            .prepare("SELECT body FROM records ORDER BY external_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let a: Value = serde_json::from_str(&values[0]).unwrap();
        let b: Value = serde_json::from_str(&values[1]).unwrap();
        assert_eq!(a["data"]["name"], b["data"]["name"]);
        assert!(store.snapshot(&snapshot).is_err());
        assert_eq!(
            store.snapshot(&archive.join("private.sqlite3")),
            Err("unsafe_location")
        );
    }
}
