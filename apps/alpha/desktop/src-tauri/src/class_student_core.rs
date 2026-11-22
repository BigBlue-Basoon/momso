use chrono::DateTime;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const MAX_BATCH: usize = 32;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Source {
    service: String,
    external_id: String,
    observed_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClassData {
    name: Option<String>,
    starts_at: Option<String>,
    ends_at: Option<String>,
    instructor: Option<String>,
    capacity: Option<u32>,
    reservation_count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PassData {
    name: Option<String>,
    remaining: Option<u32>,
    starts_at: Option<String>,
    ends_at: Option<String>,
    payment_text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StudentData {
    name: Option<String>,
    contact: Option<String>,
    passes: Option<Vec<PassData>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReservationData {
    class_external_id: Option<String>,
    student_external_id: Option<String>,
    status: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
enum Data {
    Class(ClassData),
    Student(StudentData),
    Reservation(ReservationData),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    source: Source,
    #[serde(flatten)]
    data: Data,
}

impl Record {
    fn kind(&self) -> &'static str {
        match &self.data {
            Data::Class(_) => "class",
            Data::Student(_) => "student",
            Data::Reservation(_) => "reservation",
        }
    }

    fn valid(&self) -> bool {
        let source = &self.source;
        let identifier = |s: &str| {
            !s.trim().is_empty()
                && s.len() <= 128
                && s == s.trim()
                && !s.chars().any(char::is_control)
        };
        let text = |s: &Option<String>| {
            s.as_ref()
                .is_none_or(|s| s.len() <= 1024 && !s.chars().any(char::is_control))
        };
        let time = |s: &Option<String>| {
            s.as_ref()
                .is_none_or(|s| DateTime::parse_from_rfc3339(s).is_ok())
        };
        let date_or_time = |s: &Option<String>| {
            s.as_ref().is_none_or(|s| {
                DateTime::parse_from_rfc3339(s).is_ok()
                    || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
            })
        };
        serde_json::to_vec(self).is_ok_and(|body| body.len() <= 4096)
            && identifier(&source.service)
            && valid_service(&source.service)
            && identifier(&source.external_id)
            && !source.external_id.contains("://")
            && DateTime::parse_from_rfc3339(&source.observed_at).is_ok()
            && match &self.data {
                Data::Class(d) => {
                    text(&d.name) && text(&d.instructor) && time(&d.starts_at) && time(&d.ends_at)
                }
                Data::Student(d) => {
                    text(&d.name)
                        && text(&d.contact)
                        && d.passes.as_ref().is_none_or(|passes| {
                            passes.len() <= 32
                                && passes.iter().all(|p| {
                                    text(&p.name)
                                        && text(&p.payment_text)
                                        && date_or_time(&p.starts_at)
                                        && date_or_time(&p.ends_at)
                                })
                        })
                }
                Data::Reservation(d) => {
                    text(&d.class_external_id) && text(&d.student_external_id) && text(&d.status)
                }
            }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpsertArgs {
    records: Vec<Record>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadArgs {
    kind: String,
    service: String,
    external_id: String,
    history_cursor: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchArgs {
    kind: String,
    service: Option<String>,
    query: Option<String>,
    cursor: Option<u32>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LastArgs {
    service: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MarkReadArgs {
    service: String,
    observed_at: String,
}

fn valid_kind(s: &str) -> bool {
    matches!(s, "class" | "student" | "reservation")
}

pub(crate) fn valid(method: &str, args: &Value) -> bool {
    if !serde_json::to_vec(args).is_ok_and(|body| body.len() <= 60_000) {
        return false;
    }
    match method {
        "momso_crm_upsert" => {
            args.get("records")
                .and_then(Value::as_array)
                .is_some_and(|records| {
                    records.iter().all(|record| {
                        record.as_object().is_some_and(|map| {
                            map.keys()
                                .all(|key| matches!(key.as_str(), "kind" | "source" | "data"))
                        })
                    })
                })
                && serde_json::from_value::<UpsertArgs>(args.clone()).is_ok_and(|a| {
                    !a.records.is_empty()
                        && a.records.len() <= MAX_BATCH
                        && a.records.iter().all(Record::valid)
                })
        }
        "momso_crm_read" => serde_json::from_value::<ReadArgs>(args.clone()).is_ok_and(|a| {
            valid_kind(&a.kind)
                && valid_service(&a.service)
                && valid_key(&a.external_id)
                && !a.external_id.contains("://")
                && a.history_cursor.is_none_or(|n| n <= 1_000_000)
        }),
        "momso_crm_search" => serde_json::from_value::<SearchArgs>(args.clone()).is_ok_and(|a| {
            valid_kind(&a.kind)
                && a.service.as_deref().is_none_or(valid_service)
                && a.query
                    .as_ref()
                    .is_none_or(|s| s.len() <= 200 && !s.chars().any(char::is_control))
                && a.limit.is_none_or(|n| (1..=10).contains(&n))
                && a.cursor.is_none_or(|n| n <= 1_000_000)
        }),
        "momso_crm_last_read" => serde_json::from_value::<LastArgs>(args.clone())
            .is_ok_and(|a| valid_service(&a.service)),
        "momso_crm_mark_read" => {
            serde_json::from_value::<MarkReadArgs>(args.clone()).is_ok_and(|a| {
                valid_service(&a.service) && DateTime::parse_from_rfc3339(&a.observed_at).is_ok()
            })
        }
        _ => false,
    }
}

fn valid_key(s: &str) -> bool {
    !s.trim().is_empty() && s == s.trim() && s.len() <= 128 && !s.chars().any(char::is_control)
}

fn valid_service(s: &str) -> bool {
    valid_key(s) && !s.contains(['/', '\\', '?', '#', ':'])
}

pub(crate) fn open(data_root: &Path, archive_root: &Path) -> Result<Connection, &'static str> {
    let data = data_root.canonicalize().map_err(|_| "unavailable")?;
    let archive = archive_root.canonicalize().map_err(|_| "unavailable")?;
    if data.starts_with(&archive) {
        return Err("unsafe_location");
    }
    let path = data.join("class-student-private.sqlite3");
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() {
            return Err("unsafe_location");
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("unsafe_location");
            }
        }
    }
    let db = Connection::open(path).map_err(|_| "unavailable")?;
    db.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "unavailable")?;
    db.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE IF NOT EXISTS records (kind TEXT NOT NULL, service TEXT NOT NULL, external_id TEXT NOT NULL, observed_ms INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(kind,service,external_id)); CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, service TEXT NOT NULL, external_id TEXT NOT NULL, body TEXT NOT NULL); CREATE TABLE IF NOT EXISTS source_reads (service TEXT PRIMARY KEY, observed_ms INTEGER NOT NULL, observed_at TEXT NOT NULL);").map_err(|_| "unavailable")?;
    Ok(db)
}

pub(crate) fn dispatch(data_root: &Path, archive_root: &Path, method: &str, args: &Value) -> Value {
    if !valid(method, args) {
        return json!({"status":"invalid_arguments"});
    }
    let result = open(data_root, archive_root).and_then(|mut db| run(&mut db, method, args));
    result.unwrap_or_else(|status| json!({"status":status}))
}

fn run(db: &mut Connection, method: &str, args: &Value) -> Result<Value, &'static str> {
    match method {
        "momso_crm_upsert" => {
            let input: UpsertArgs =
                serde_json::from_value(args.clone()).map_err(|_| "invalid_arguments")?;
            let mut keys = std::collections::HashSet::new();
            for r in &input.records {
                if !keys.insert((r.kind(), &r.source.service, &r.source.external_id)) {
                    return Err("duplicate_key");
                }
            }
            let tx = db
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| "unavailable")?;
            for r in &input.records {
                let source = &r.source;
                let ms = DateTime::parse_from_rfc3339(&source.observed_at)
                    .map_err(|_| "invalid_arguments")?
                    .timestamp_millis();
                let old: Option<(i64, String)> = tx.query_row("SELECT observed_ms,body FROM records WHERE kind=?1 AND service=?2 AND external_id=?3", params![r.kind(), source.service, source.external_id], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|_| "unavailable")?;
                if old.as_ref().is_some_and(|(old_ms, _)| *old_ms > ms) {
                    return Err("stale_observation");
                }
                let body = serde_json::to_string(r).map_err(|_| "invalid_arguments")?;
                if let Some((_, old_body)) = old {
                    if old_body != body {
                        tx.execute("INSERT INTO history(kind,service,external_id,body) VALUES(?1,?2,?3,?4)", params![r.kind(), source.service, source.external_id, old_body]).map_err(|_| "unavailable")?;
                    }
                }
                tx.execute("INSERT INTO records(kind,service,external_id,observed_ms,body) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(kind,service,external_id) DO UPDATE SET observed_ms=excluded.observed_ms,body=excluded.body", params![r.kind(), source.service, source.external_id, ms, body]).map_err(|_| "unavailable")?;
                tx.execute("INSERT INTO source_reads(service,observed_ms,observed_at) VALUES(?1,?2,?3) ON CONFLICT(service) DO UPDATE SET observed_ms=excluded.observed_ms,observed_at=excluded.observed_at WHERE excluded.observed_ms > source_reads.observed_ms", params![source.service,ms,source.observed_at]).map_err(|_| "unavailable")?;
            }
            tx.commit().map_err(|_| "unavailable")?;
            Ok(json!({"status":"ok","count":input.records.len()}))
        }
        "momso_crm_read" => {
            let a: ReadArgs =
                serde_json::from_value(args.clone()).map_err(|_| "invalid_arguments")?;
            let body: Option<String> = db
                .query_row(
                    "SELECT body FROM records WHERE kind=?1 AND service=?2 AND external_id=?3",
                    params![a.kind, a.service, a.external_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| "unavailable")?;
            let Some(body) = body else {
                return Ok(json!({"status":"not_found"}));
            };
            let cursor = a.history_cursor.unwrap_or(0);
            let mut stmt = db.prepare("SELECT body FROM history WHERE kind=?1 AND service=?2 AND external_id=?3 ORDER BY id DESC LIMIT 11 OFFSET ?4").map_err(|_| "unavailable")?;
            let history = stmt
                .query_map(params![a.kind, a.service, a.external_id, cursor], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|_| "unavailable")?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "unavailable")?;
            let more = history.len() > 10;
            let history: Vec<Value> = history
                .iter()
                .take(10)
                .map(|s| serde_json::from_str(s).map_err(|_| "unavailable"))
                .collect::<Result<_, _>>()?;
            Ok(
                json!({"status":"ok","record":serde_json::from_str::<Value>(&body).map_err(|_| "unavailable")?,"history":history,"nextHistoryCursor":if more {Some(cursor+10)} else {None}}),
            )
        }
        "momso_crm_search" => {
            let a: SearchArgs =
                serde_json::from_value(args.clone()).map_err(|_| "invalid_arguments")?;
            let limit = a.limit.unwrap_or(10);
            let offset = a.cursor.unwrap_or(0);
            let mut stmt = db.prepare("SELECT body FROM records WHERE kind=?1 AND (?2 IS NULL OR service=?2) AND (?3 IS NULL OR instr(lower(body), lower(?3))>0) ORDER BY service,external_id LIMIT ?4 OFFSET ?5").map_err(|_| "unavailable")?;
            let rows = stmt
                .query_map(
                    params![a.kind, a.service, a.query, limit + 1, offset],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|_| "unavailable")?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "unavailable")?;
            let has_more = rows.len() > limit as usize;
            let records: Vec<Value> = rows
                .iter()
                .take(limit as usize)
                .map(|s| serde_json::from_str(s).map_err(|_| "unavailable"))
                .collect::<Result<_, _>>()?;
            Ok(
                json!({"status":"ok","records":records,"nextCursor":if has_more {Some(offset + limit)} else {None}}),
            )
        }
        "momso_crm_last_read" => {
            let a: LastArgs =
                serde_json::from_value(args.clone()).map_err(|_| "invalid_arguments")?;
            let observed: Option<String> = db
                .query_row(
                    "SELECT observed_at FROM source_reads WHERE service=?1",
                    params![a.service],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| "unavailable")?;
            Ok(json!({"status":"ok","lastReadAt":observed}))
        }
        "momso_crm_mark_read" => {
            let a: MarkReadArgs =
                serde_json::from_value(args.clone()).map_err(|_| "invalid_arguments")?;
            let ms = DateTime::parse_from_rfc3339(&a.observed_at)
                .map_err(|_| "invalid_arguments")?
                .timestamp_millis();
            db.execute("INSERT INTO source_reads(service,observed_ms,observed_at) VALUES(?1,?2,?3) ON CONFLICT(service) DO UPDATE SET observed_ms=excluded.observed_ms,observed_at=excluded.observed_at WHERE excluded.observed_ms > source_reads.observed_ms", params![a.service,ms,a.observed_at]).map_err(|_| "unavailable")?;
            Ok(json!({"status":"ok"}))
        }
        _ => Err("invalid_arguments"),
    }
}
