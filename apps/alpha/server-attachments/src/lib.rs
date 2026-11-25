//! Host-owned resumable attachment storage. HTTP authentication and chat
//! ownership checks are the server's responsibility, never the model's.
#![cfg(target_os = "linux")]
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
use uuid::Uuid;

// SQLite stores lengths as signed 64-bit integers.
pub const MAX_BYTES: u64 = i64::MAX as u64;
pub const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
pub const DATABASE_NAME: &str = "attachments.sqlite3";
const UPLOAD_STALE_SECONDS: u64 = 24 * 60 * 60;
pub type AssetDate = (Option<(String, String)>, bool);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub upload_id: String,
    pub received: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub sha256: String,
    pub byte_length: u64,
    pub original_file_name: String,
    pub media_type: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Generation {
    pub asset_id: String,
    pub prompt: String,
    pub prompt_source: String,
    pub provider: String,
    pub model: Option<String>,
    pub created_at: String,
    pub chat_id: String,
    pub message_id: String,
    pub lineage_id: String,
    pub version: i64,
    pub version_of: Option<String>,
    pub version_how: Option<String>,
}

pub struct GenerationInput<'a> {
    pub asset_id: &'a str,
    pub prompt: &'a str,
    pub prompt_source: &'a str,
    pub provider: &'a str,
    pub model: Option<&'a str>,
    pub created_at: &'a str,
    pub chat_id: &'a str,
    pub message_id: &'a str,
    pub parent: Option<&'a str>,
    pub how: Option<&'a str>,
}

/// The host must bind this decision to the authenticated caller and actual
/// server chat. Do not implement it from a request's self-declared owner field.
pub trait ChatAccess {
    fn may_access(&self, chat_id: &str) -> bool;
}

pub struct Store {
    data: PathBuf,
    objects: PathBuf,
}

fn uuid(input: &str) -> Result<(), &'static str> {
    let parsed = Uuid::parse_str(input).map_err(|_| "invalid_id")?;
    if parsed.to_string() != input {
        return Err("invalid_id");
    }
    Ok(())
}

fn write_error(error: std::io::Error) -> &'static str {
    if error.raw_os_error() == Some(libc::ENOSPC) {
        "disk_low"
    } else {
        "storage_unavailable"
    }
}

fn sqlite_write_error(error: rusqlite::Error) -> &'static str {
    match error {
        rusqlite::Error::SqliteFailure(code, _)
            if code.code == rusqlite::ffi::ErrorCode::DiskFull =>
        {
            "disk_low"
        }
        _ => "storage_unavailable",
    }
}

fn enough_space(stage: u128, archive: u128, same_device: bool, bytes: u64) -> bool {
    let required = u128::from(bytes);
    if same_device {
        stage >= required * 2
    } else {
        stage >= required && archive >= required
    }
}

fn available_space(path: &Path) -> Result<u128, &'static str> {
    let path =
        std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| "unsafe_location")?;
    let mut value = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // statvfs initializes the output only on success.
    if unsafe { libc::statvfs(path.as_ptr(), value.as_mut_ptr()) } != 0 {
        return Err("storage_unavailable");
    }
    let value = unsafe { value.assume_init() };
    Ok(u128::from(value.f_bavail) * u128::from(value.f_frsize))
}

fn nonnegative(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
fn safe_dir(path: &Path) -> Result<(), &'static str> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("storage_unavailable"),
    }
    let m = fs::symlink_metadata(path).map_err(|_| "storage_unavailable")?;
    if !m.file_type().is_dir() {
        return Err("unsafe_location");
    }
    Ok(())
}
fn file(path: &Path, write: bool) -> Result<File, &'static str> {
    let mut o = OpenOptions::new();
    o.read(true)
        .write(write)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let f = o.open(path).map_err(|_| "storage_unavailable")?;
    if !f.metadata().map_err(|_| "storage_unavailable")?.is_file() {
        return Err("unsafe_location");
    }
    Ok(f)
}
fn digest(mut f: &File) -> Result<(String, u64), &'static str> {
    let mut h = Sha256::new();
    let mut n = 0u64;
    let mut buf = [0u8; 65536];
    loop {
        let read = f.read(&mut buf).map_err(|_| "storage_unavailable")?;
        if read == 0 {
            break;
        }
        h.update(&buf[..read]);
        n += read as u64;
    }
    Ok((hex::encode(h.finalize()), n))
}
fn valid_meta(name: &str, mime: &str, total: u64) -> bool {
    total <= MAX_BYTES
        && !name.trim().is_empty()
        && name.chars().count() <= 255
        && !name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        && !mime.is_empty()
        && mime.len() <= 120
        && mime.is_ascii()
        && !mime.bytes().any(|c| c.is_ascii_control())
}
fn kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "image"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime.starts_with("video/") {
        "video"
    } else if mime.starts_with("text/") {
        "text"
    } else {
        "other"
    }
}

impl Store {
    pub fn mark_generated_upload(
        &self,
        upload: &str,
        chat: &str,
        message: &str,
        provider: &str,
    ) -> Result<(), &'static str> {
        uuid(upload)?;
        if chat.is_empty()
            || message.is_empty()
            || !matches!(provider, "codex" | "claude" | "gemini")
        {
            return Err("invalid_arguments");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let changed=c.execute("INSERT INTO generated_uploads(upload_id,chat_id,message_id,provider) SELECT id,?2,?3,?4 FROM uploads WHERE id=?1 AND asset_id IS NULL",params![upload,chat,message,provider]).map_err(|_|"storage_unavailable")?;
        if changed != 1 {
            return Err("not_found");
        }
        Ok(())
    }
    pub fn owns_generated_asset(
        &self,
        asset: &str,
        chat: &str,
        message: &str,
        provider: &str,
    ) -> Result<bool, &'static str> {
        uuid(asset)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        c.query_row("SELECT EXISTS(SELECT 1 FROM generated_uploads g JOIN uploads u ON u.id=g.upload_id WHERE u.asset_id=?1 AND g.chat_id=?2 AND g.message_id=?3 AND g.provider=?4 AND g.created_asset=1)",params![asset,chat,message,provider],|r|r.get(0)).map_err(|_|"storage_unavailable")
    }
    pub fn find_hash(&self, hash: &str) -> Result<Option<Asset>, &'static str> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid_arguments");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let id: Option<String> = c
            .query_row("SELECT id FROM assets WHERE hash=?1", [hash], |r| r.get(0))
            .optional()
            .map_err(|_| "storage_unavailable")?;
        id.map(|id| asset_by_id(&c, &id)).transpose()
    }
    pub fn record_generation(
        &self,
        input: GenerationInput<'_>,
    ) -> Result<Generation, &'static str> {
        uuid(input.asset_id)?;
        if input.prompt.len() > 65536
            || !matches!(input.prompt_source, "reported" | "userRequest")
            || !matches!(input.provider, "codex" | "claude" | "gemini")
            || input
                .model
                .is_some_and(|v| v.len() > 200 || v.chars().any(char::is_control))
            || input.chat_id.is_empty()
            || input.message_id.is_empty()
            || input.created_at.is_empty()
            || input
                .how
                .is_some_and(|v| !matches!(v, "regenerated" | "edited"))
        {
            return Err("invalid_arguments");
        }
        if let Some(parent) = input.parent {
            uuid(parent)?;
        }
        let _lock = self.lock()?;
        let mut c = self.db()?;
        let tx = c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "storage_unavailable")?;
        let asset = asset_by_id(&tx, input.asset_id)?;
        if !matches!(asset.kind.as_str(), "image" | "video") || trashed(&tx, input.asset_id)? {
            return Err("invalid_arguments");
        }
        let (_, mut source) = self.open_verified(asset.clone())?;
        if momso_core::media_format::classify_file(&mut source)
            .map_err(|_| "storage_unavailable")?
            .map(|(kind, _)| kind)
            != Some(asset.kind.as_str())
        {
            return Err("invalid_arguments");
        }
        if let Some(existing) = generation_by_id(&tx, input.asset_id)? {
            if existing.chat_id == input.chat_id
                && existing.message_id == input.message_id
                && existing.provider == input.provider
            {
                return Ok(existing);
            }
            return Err("already_recorded");
        }
        let lineage = if let Some(parent) = input.parent {
            let prior = generation_by_id(&tx, parent)?
                .filter(|v| v.chat_id == input.chat_id)
                .ok_or("parent_not_found")?;
            let parent_asset = asset_by_id(&tx, parent)?;
            if parent_asset.kind != asset.kind {
                return Err("parent_not_found");
            }
            prior.lineage_id
        } else if input.prompt_source == "reported" && !input.prompt.trim().is_empty() {
            tx.query_row("SELECT g.lineage_id FROM media_generation g JOIN assets a ON a.id=g.asset_id WHERE g.chat_id=?1 AND g.prompt=?2 AND g.prompt_source='reported' AND a.kind=?3 ORDER BY g.version LIMIT 1",params![input.chat_id,input.prompt,asset.kind],|r|r.get::<_,String>(0)).optional().map_err(|_|"storage_unavailable")?.unwrap_or_else(||Uuid::new_v4().to_string())
        } else {
            Uuid::new_v4().to_string()
        };
        let version: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(version),0)+1 FROM media_generation WHERE lineage_id=?1",
                [&lineage],
                |r| r.get(0),
            )
            .map_err(|_| "storage_unavailable")?;
        let how = if version > 1 {
            Some(input.how.unwrap_or("regenerated"))
        } else {
            None
        };
        tx.execute("INSERT INTO media_generation(asset_id,prompt,prompt_source,provider,model,created_at,chat_id,message_id,lineage_id,version,version_of,version_how) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![input.asset_id,input.prompt,input.prompt_source,input.provider,input.model,input.created_at,input.chat_id,input.message_id,lineage,version,input.parent,how]).map_err(|_|"storage_unavailable")?;
        let result = generation_by_id(&tx, input.asset_id)?.ok_or("storage_unavailable")?;
        tx.commit().map_err(|_| "storage_unavailable")?;
        Ok(result)
    }

    pub fn generation(&self, id: &str) -> Result<Option<Generation>, &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        generation_by_id(&self.db()?, id)
    }

    pub fn versions(&self, lineage: &str) -> Result<Vec<Generation>, &'static str> {
        uuid(lineage)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        let mut statement = c
            .prepare("SELECT asset_id FROM media_generation WHERE lineage_id=?1 ORDER BY version")
            .map_err(|_| "storage_unavailable")?;
        let ids = statement
            .query_map([lineage], |r| r.get::<_, String>(0))
            .map_err(|_| "storage_unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_unavailable")?;
        ids.iter()
            .map(|id| generation_by_id(&c, id)?.ok_or("storage_unavailable"))
            .collect()
    }
    /// Trash is a reversible marker: immutable originals and chat links remain.
    pub fn set_trashed(&self, id: &str, deleted_at: Option<&str>) -> Result<bool, &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        asset_by_id(&c, id)?;
        if deleted_at.is_none()
            && c.query_row(
                "SELECT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
                [id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|_| "storage_unavailable")?
        {
            return Err("asset_purged");
        }
        let changed = if let Some(time) = deleted_at {
            if time.is_empty() || time.len() > 64 || time.chars().any(char::is_control) {
                return Err("invalid_arguments");
            }
            c.execute(
                "INSERT OR IGNORE INTO asset_trash(asset_id,deleted_at) VALUES(?1,?2)",
                params![id, time],
            )
            .map_err(|_| "storage_unavailable")?
        } else {
            c.execute("DELETE FROM asset_trash WHERE asset_id=?1", [id])
                .map_err(|_| "storage_unavailable")?
        };
        Ok(changed != 0)
    }

    pub fn is_trashed(&self, id: &str) -> Result<bool, &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        asset_by_id(&c, id)?;
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM asset_trash WHERE asset_id=?1) OR EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
            [id],
            |r| r.get(0),
        )
        .map_err(|_| "storage_unavailable")
    }

    pub fn linked_chats(&self, id: &str) -> Result<Vec<String>, &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        asset_by_id(&c, id)?;
        let mut query = c
            .prepare("SELECT chat_id FROM attachments WHERE asset_id=?1")
            .map_err(|_| "storage_unavailable")?;
        let rows = query
            .query_map([id], |r| r.get(0))
            .map_err(|_| "storage_unavailable")?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_unavailable")
    }

    pub fn trashed_assets(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<(Asset, String)>, &'static str> {
        if limit == 0 || limit > 100 {
            return Err("invalid_arguments");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let mut statement = c.prepare("SELECT asset_id,deleted_at FROM asset_trash WHERE NOT EXISTS(SELECT 1 FROM asset_purged WHERE asset_purged.asset_id=asset_trash.asset_id) ORDER BY deleted_at DESC,asset_id LIMIT ?1 OFFSET ?2").map_err(|_| "storage_unavailable")?;
        let rows = statement
            .query_map(params![limit, offset], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|_| "storage_unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_unavailable")?;
        rows.into_iter()
            .map(|(id, time)| Ok((asset_by_id(&c, &id)?, time)))
            .collect()
    }

    /// Search registered originals by their recorded file name.
    pub fn search_assets(
        &self,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Asset>, &'static str> {
        let query = query.trim();
        if query.is_empty()
            || query.chars().count() > 200
            || query.chars().any(char::is_control)
            || limit == 0
            || limit > 100
        {
            return Err("invalid_arguments");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let mut statement = c.prepare("SELECT id FROM assets WHERE instr(lower(name),lower(?1))>0 AND NOT EXISTS(SELECT 1 FROM asset_trash WHERE asset_id=assets.id) AND NOT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=assets.id) ORDER BY rowid DESC LIMIT ?2 OFFSET ?3").map_err(|_| "storage_unavailable")?;
        let ids = statement
            .query_map(params![query, limit, offset], |r| r.get::<_, String>(0))
            .map_err(|_| "storage_unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_unavailable")?;
        drop(statement);
        ids.iter().map(|id| asset_by_id(&c, id)).collect()
    }

    /// Enumerate registered assets without reading or changing original files.
    pub fn list_media(
        &self,
        kind_filter: Option<&str>,
        chat: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Asset>, &'static str> {
        if !matches!(kind_filter, None | Some("image") | Some("video")) || limit == 0 || limit > 100
        {
            return Err("invalid_arguments");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let mut statement = c.prepare("SELECT id FROM assets WHERE kind IN ('image','video') AND NOT EXISTS(SELECT 1 FROM asset_trash WHERE asset_id=assets.id) AND NOT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=assets.id) AND (?1 IS NULL OR kind=?1) AND (?2 IS NULL OR EXISTS(SELECT 1 FROM attachments WHERE chat_id=?2 AND asset_id=assets.id) OR EXISTS(SELECT 1 FROM media_generation WHERE asset_id=assets.id AND chat_id=?2)) AND NOT EXISTS(SELECT 1 FROM media_generation old JOIN media_generation newer ON newer.lineage_id=old.lineage_id AND newer.version>old.version WHERE old.asset_id=assets.id AND NOT EXISTS(SELECT 1 FROM asset_trash WHERE asset_id=newer.asset_id) AND NOT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=newer.asset_id)) ORDER BY rowid DESC LIMIT ?3 OFFSET ?4").map_err(|_| "storage_unavailable")?;
        let ids = statement
            .query_map(params![kind_filter, chat, limit, offset], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|_| "storage_unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_unavailable")?;
        drop(statement);
        ids.iter().map(|id| asset_by_id(&c, id)).collect()
    }

    /// Both roots are created and owned by the host, never supplied by a phone.
    pub fn open(data: &Path, archive: &Path) -> Result<Self, &'static str> {
        let data = data.canonicalize().map_err(|_| "unsafe_location")?;
        let archive = archive.canonicalize().map_err(|_| "unsafe_location")?;
        if data.starts_with(&archive) || archive.starts_with(&data) {
            return Err("unsafe_location");
        }
        let objects = archive.join("objects");
        safe_dir(&objects)?;
        safe_dir(&objects.join("sha256"))?;
        safe_dir(&data.join("upload-staging"))?;
        let s = Self { data, objects };
        let _lock = s.lock()?;
        let c = s.db()?;
        c.execute_batch("PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS uploads(id TEXT PRIMARY KEY,total INTEGER NOT NULL,received INTEGER NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,expected TEXT,asset_id TEXT); CREATE TABLE IF NOT EXISTS assets(id TEXT PRIMARY KEY,hash TEXT UNIQUE NOT NULL,bytes INTEGER NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,kind TEXT NOT NULL); CREATE TABLE IF NOT EXISTS attachments(chat_id TEXT NOT NULL,asset_id TEXT NOT NULL REFERENCES assets(id),PRIMARY KEY(chat_id,asset_id));").map_err(|_| "storage_unavailable")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS asset_trash(asset_id TEXT PRIMARY KEY REFERENCES assets(id), deleted_at TEXT NOT NULL)").map_err(|_| "storage_unavailable")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS asset_purged(asset_id TEXT PRIMARY KEY REFERENCES assets(id))").map_err(|_| "storage_unavailable")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS asset_dates(asset_id TEXT PRIMARY KEY REFERENCES assets(id), created_at TEXT NOT NULL, source TEXT NOT NULL)").map_err(|_| "storage_unavailable")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS media_generation(asset_id TEXT PRIMARY KEY REFERENCES assets(id),prompt TEXT NOT NULL,prompt_source TEXT NOT NULL,provider TEXT NOT NULL,model TEXT,created_at TEXT NOT NULL,chat_id TEXT NOT NULL,message_id TEXT NOT NULL,lineage_id TEXT NOT NULL,version INTEGER NOT NULL,version_of TEXT,version_how TEXT,UNIQUE(lineage_id,version))").map_err(|_|"storage_unavailable")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS generated_uploads(upload_id TEXT PRIMARY KEY REFERENCES uploads(id),chat_id TEXT NOT NULL,message_id TEXT NOT NULL,provider TEXT NOT NULL,created_asset INTEGER NOT NULL DEFAULT 0)").map_err(|_|"storage_unavailable")?;
        Ok(s)
    }
    fn db(&self) -> Result<Connection, &'static str> {
        let path = self.data.join(DATABASE_NAME);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(f) => {
                f.sync_all().map_err(|_| "storage_unavailable")?;
                File::open(&self.data)
                    .and_then(|d| d.sync_all())
                    .map_err(|_| "storage_unavailable")?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("storage_unavailable"),
        }
        let meta = fs::symlink_metadata(&path).map_err(|_| "storage_unavailable")?;
        if !meta.file_type().is_file() || meta.permissions().mode() & 0o077 != 0 {
            return Err("unsafe_location");
        }
        let c = Connection::open(path).map_err(|_| "storage_unavailable")?;
        c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")
            .map_err(|_| "storage_unavailable")?;
        Ok(c)
    }
    fn lock(&self) -> Result<File, &'static str> {
        let f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.data.join("attachments.lock"))
            .map_err(|_| "storage_unavailable")?;
        f.lock_exclusive().map_err(|_| "storage_unavailable")?;
        Ok(f)
    }
    fn stage(&self, id: &str) -> PathBuf {
        self.data.join("upload-staging").join(id)
    }
    fn object(&self, hash: &str) -> PathBuf {
        self.objects.join("sha256").join(&hash[..2]).join(hash)
    }

    fn prune_stale_uploads(&self) -> Result<(), &'static str> {
        let db = self.db()?;
        let mut statement = db
            .prepare("SELECT id FROM uploads WHERE asset_id IS NULL")
            .map_err(|_| "storage_unavailable")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| "storage_unavailable")?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "storage_unavailable")?;
        drop(statement);
        for id in ids {
            if uuid(&id).is_err() {
                continue;
            }
            let path = self.stage(&id);
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            let Ok(age) = metadata
                .modified()
                .and_then(|t| t.elapsed().map_err(std::io::Error::other))
            else {
                continue;
            };
            if age.as_secs() < UPLOAD_STALE_SECONDS {
                continue;
            }
            fs::remove_file(path).map_err(|_| "storage_unavailable")?;
            db.execute("DELETE FROM generated_uploads WHERE upload_id=?1", [&id])
                .map_err(|_| "storage_unavailable")?;
            db.execute(
                "DELETE FROM uploads WHERE id=?1 AND asset_id IS NULL",
                [&id],
            )
            .map_err(|_| "storage_unavailable")?;
        }
        Ok(())
    }

    pub fn cleanup_expired_uploads(&self) -> Result<(), &'static str> {
        let _lock = self.lock()?;
        self.prune_stale_uploads()
    }

    /// Expects authenticated host. Expected SHA-256 is optional, but if sent it
    /// must match the complete upload; a mismatch is never imported.
    pub fn begin(
        &self,
        name: &str,
        mime: &str,
        total: u64,
        expected: Option<&str>,
    ) -> Result<Progress, &'static str> {
        if !valid_meta(name, mime, total)
            || expected.is_some_and(|h| {
                h.len() != 64
                    || !h
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            })
        {
            return Err("invalid_arguments");
        }
        let _lock = self.lock()?;
        self.prune_stale_uploads()?;
        let same_device = fs::metadata(&self.data)
            .map_err(|_| "storage_unavailable")?
            .dev()
            == fs::metadata(&self.objects)
                .map_err(|_| "storage_unavailable")?
                .dev();
        if !enough_space(
            available_space(&self.data)?,
            available_space(&self.objects)?,
            same_device,
            total,
        ) {
            return Err("disk_low");
        }
        let id = Uuid::new_v4().to_string();
        let f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.stage(&id))
            .map_err(write_error)?;
        f.sync_all().map_err(write_error)?;
        File::open(self.data.join("upload-staging"))
            .and_then(|d| d.sync_all())
            .map_err(|_| "storage_unavailable")?;
        self.db()?.execute("INSERT INTO uploads(id,total,received,name,mime,expected) VALUES(?1,?2,0,?3,?4,?5)",params![id,total as i64,name,mime,expected]).map_err(sqlite_write_error)?;
        Ok(Progress {
            upload_id: id,
            received: 0,
            total,
            sha256: expected.map(str::to_owned),
        })
    }
    pub fn progress(&self, id: &str) -> Result<Progress, &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        let progress = self
            .db()?
            .query_row(
                "SELECT received,total,expected FROM uploads WHERE id=?1",
                [id],
                |r| {
                    Ok(Progress {
                        upload_id: id.to_owned(),
                        received: nonnegative(r, 0)?,
                        total: nonnegative(r, 1)?,
                        sha256: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|_| "storage_unavailable")?
            .ok_or("not_found")?;
        if progress.total > MAX_BYTES || progress.received > progress.total {
            return Err("integrity_error");
        }
        Ok(progress)
    }
    /// Caller retries only from the server-reported offset. Data is durable
    /// before advancing it; unacknowledged trailing bytes are discarded on retry.
    pub fn write_chunk(
        &self,
        id: &str,
        offset: u64,
        bytes: &[u8],
    ) -> Result<Progress, &'static str> {
        uuid(id)?;
        if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES {
            return Err("invalid_chunk");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let (received, total, finished, expected): (u64, u64, Option<String>, Option<String>) = c
            .query_row(
                "SELECT received,total,asset_id,expected FROM uploads WHERE id=?1",
                [id],
                |r| Ok((nonnegative(r, 0)?, nonnegative(r, 1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
            .map_err(|_| "storage_unavailable")?
            .ok_or("not_found")?;
        if total > MAX_BYTES || received > total {
            return Err("integrity_error");
        }
        if finished.is_some() {
            return Err("already_complete");
        }
        if offset != received {
            return Err("offset_mismatch");
        }
        if received
            .checked_add(bytes.len() as u64)
            .is_none_or(|n| n > total)
        {
            return Err("invalid_chunk");
        }
        let mut f = file(&self.stage(id), true)?;
        let actual = f.metadata().map_err(|_| "storage_unavailable")?.len();
        if actual < received {
            return Err("integrity_error");
        }
        if actual > received {
            f.set_len(received).map_err(|_| "storage_unavailable")?;
        }
        f.seek(SeekFrom::Start(received))
            .and_then(|_| f.write_all(bytes))
            .and_then(|_| f.sync_all())
            .map_err(write_error)?;
        let next = received + bytes.len() as u64;
        c.execute(
            "UPDATE uploads SET received=?1 WHERE id=?2",
            params![next as i64, id],
        )
        .map_err(sqlite_write_error)?;
        Ok(Progress {
            upload_id: id.to_owned(),
            received: next,
            total,
            sha256: expected,
        })
    }

    pub fn finish(&self, id: &str) -> Result<Asset, &'static str> {
        self.finish_with_created(id).map(|(asset, _)| asset)
    }

    pub fn finish_with_created(&self, id: &str) -> Result<(Asset, bool), &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        let mut c = self.db()?;
        let (received, total, name, mime, expected, finished): (
            u64,
            u64,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = c
            .query_row(
                "SELECT received,total,name,mime,expected,asset_id FROM uploads WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        nonnegative(r, 0)?,
                        nonnegative(r, 1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| "storage_unavailable")?
            .ok_or("not_found")?;
        if total > MAX_BYTES || received > total {
            return Err("integrity_error");
        }
        if let Some(asset) = finished {
            if c.query_row(
                "SELECT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
                [&asset],
                |r| r.get::<_, bool>(0),
            )
            .map_err(|_| "storage_unavailable")?
            {
                return Err("asset_purged");
            }
            return Ok((asset_by_id(&c, &asset)?, false));
        }
        if received != total {
            return Err("incomplete");
        }
        let stage = self.stage(id);
        let mut f = file(&stage, false)?;
        let (hash, bytes) = digest(&f)?;
        if bytes != total || expected.as_ref().is_some_and(|e| e != &hash) {
            return Err("integrity_error");
        }
        if c.query_row("SELECT EXISTS(SELECT 1 FROM assets JOIN asset_purged ON asset_purged.asset_id=assets.id WHERE assets.hash=?1)", [&hash], |r| r.get::<_, bool>(0)).map_err(|_| "storage_unavailable")? { return Err("asset_purged"); }
        let dir = self.objects.join("sha256").join(&hash[..2]);
        safe_dir(&dir)?;
        let destination = self.object(&hash);
        // Publishing never replaces an existing object; interrupted publication
        // is safely retried after verifying the existing contents.
        // Stage and archive may be on different file systems. Copy to a unique
        // temporary file beside the destination, then publish without replacing.
        f.seek(SeekFrom::Start(0))
            .map_err(|_| "storage_unavailable")?;
        let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|_| "storage_unavailable")?;
        std::io::copy(&mut f, &mut temp).map_err(write_error)?;
        temp.flush().map_err(write_error)?;
        let (copied_hash, copied_bytes) = digest(&file(temp.path(), false)?)?;
        if copied_hash != hash || copied_bytes != bytes {
            return Err("integrity_error");
        }
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o444))
            .map_err(|_| "storage_unavailable")?;
        temp.as_file()
            .sync_all()
            .map_err(|_| "storage_unavailable")?;
        match fs::hard_link(temp.path(), &destination) {
            Ok(()) => File::open(&dir)
                .and_then(|d| d.sync_all())
                .map_err(|_| "storage_unavailable")?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = file(&destination, false)?;
                let (h, n) = digest(&existing)?;
                if h != hash || n != bytes {
                    return Err("integrity_error");
                }
                existing
                    .set_permissions(fs::Permissions::from_mode(0o444))
                    .map_err(|_| "storage_unavailable")?;
                existing.sync_all().map_err(|_| "storage_unavailable")?;
            }
            Err(_) => return Err("storage_unavailable"),
        }
        let asset_id = Uuid::new_v4().to_string();
        let tx = c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "storage_unavailable")?;
        tx.execute("INSERT INTO assets(id,hash,bytes,name,mime,kind) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(hash) DO NOTHING",params![asset_id,hash,bytes as i64,name,mime,kind(&mime)]).map_err(|_| "storage_unavailable")?;
        let asset: String = tx
            .query_row("SELECT id FROM assets WHERE hash=?1", [&hash], |r| r.get(0))
            .map_err(|_| "storage_unavailable")?;
        tx.execute(
            "UPDATE uploads SET asset_id=?1 WHERE id=?2",
            params![asset, id],
        )
        .map_err(|_| "storage_unavailable")?;
        let created = asset == asset_id;
        if created {
            tx.execute(
                "INSERT INTO asset_dates(asset_id,created_at,source) VALUES(?1,?2,'registered')",
                params![asset, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(|_| "storage_unavailable")?;
        }
        tx.execute(
            "UPDATE generated_uploads SET created_asset=?2 WHERE upload_id=?1",
            params![id, created],
        )
        .map_err(|_| "storage_unavailable")?;
        let result = asset_by_id(&tx, &asset)?;
        tx.commit().map_err(|_| "storage_unavailable")?;
        let _ = fs::remove_file(stage);
        Ok((result, created))
    }

    /// Host has verified this chat exists and is owned by the caller.
    pub fn attach(
        &self,
        access: &impl ChatAccess,
        chat: &str,
        asset: &str,
    ) -> Result<(), &'static str> {
        self.attach_impl(access, chat, asset, false)
    }

    /// Restore only the link; it must not undo a user's trash decision.
    pub fn attach_restored(
        &self,
        access: &impl ChatAccess,
        chat: &str,
        asset: &str,
    ) -> Result<(), &'static str> {
        self.attach_impl(access, chat, asset, true)
    }

    fn attach_impl(
        &self,
        access: &impl ChatAccess,
        chat: &str,
        asset: &str,
        restoring: bool,
    ) -> Result<(), &'static str> {
        uuid(asset)?;
        if chat.is_empty()
            || chat.len() > 200
            || chat.chars().any(char::is_control)
            || !access.may_access(chat)
        {
            return Err("not_found");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        asset_by_id(&c, asset)?;
        if !restoring && trashed(&c, asset)? {
            return Err("asset_trashed");
        }
        c.execute(
            "INSERT OR IGNORE INTO attachments(chat_id,asset_id) VALUES(?1,?2)",
            params![chat, asset],
        )
        .map_err(|_| "storage_unavailable")?;
        Ok(())
    }

    /// Authenticated download and AI read share this exact access check. The
    /// returned open handle is seekable for bounded streaming/range responses.
    pub fn open_attached(
        &self,
        access: &impl ChatAccess,
        chat: &str,
        asset: &str,
    ) -> Result<(Asset, File), &'static str> {
        uuid(asset)?;
        if !access.may_access(chat) {
            return Err("not_found");
        }
        let _lock = self.lock()?;
        let c = self.db()?;
        let allowed: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM attachments WHERE chat_id=?1 AND asset_id=?2)",
                params![chat, asset],
                |r| r.get(0),
            )
            .map_err(|_| "storage_unavailable")?;
        if !allowed {
            return Err("not_found");
        }
        let meta = asset_by_id(&c, asset)?;
        if trashed(&c, asset)? {
            return Err("asset_trashed");
        }
        self.open_verified(meta)
    }

    /// Original registration time when known; older assets use the actual file's
    /// modified time and explicitly say where that fallback came from.
    pub fn asset_date(&self, id: &str) -> Result<AssetDate, &'static str> {
        uuid(id)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        let existing = c
            .query_row(
                "SELECT created_at,source FROM asset_dates WHERE asset_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| "storage_unavailable")?;
        if existing.is_some() {
            return Ok((existing, false));
        }
        let (_, file) = self.open_verified(asset_by_id(&c, id)?)?;
        let Ok(time) = file.metadata().and_then(|m| m.modified()) else {
            return Ok((None, false));
        };
        let date = chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339();
        c.execute("INSERT OR IGNORE INTO asset_dates(asset_id,created_at,source) VALUES(?1,?2,'file_modified')", params![id,date]).map_err(|_| "storage_unavailable")?;
        Ok((Some((date, "file_modified".into())), true))
    }

    /// Validate a restored host-owned asset before linking it to a chat.
    pub fn verify_asset(&self, asset: &str) -> Result<(Asset, File), &'static str> {
        uuid(asset)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        let meta = asset_by_id(&c, asset)?;
        self.open_verified(meta)
    }

    /// Metadata only, for preserving a deleted attachment in an old chat.
    pub fn purged_asset(&self, asset: &str) -> Result<Option<Asset>, &'static str> {
        uuid(asset)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        let purged: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
                [asset],
                |r| r.get(0),
            )
            .map_err(|_| "storage_unavailable")?;
        if purged {
            Ok(Some(asset_by_id(&c, asset)?))
        } else {
            Ok(None)
        }
    }

    /// Call only after the backup deletion ledger has durably recorded this hash.
    /// The caller must also prevent new references while checking usage.
    pub fn mark_purged(&self, asset: &str, expected_hash: &str) -> Result<(), &'static str> {
        uuid(asset)?;
        let _lock = self.lock()?;
        let mut c = self.db()?;
        let meta = asset_by_id(&c, asset)?;
        if meta.sha256 != expected_hash {
            return Err("integrity_error");
        }
        if !trashed(&c, asset)? {
            return Err("asset_not_trashed");
        }
        let tx = c
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| "storage_unavailable")?;
        tx.execute(
            "INSERT OR IGNORE INTO asset_purged(asset_id) VALUES(?1)",
            [asset],
        )
        .map_err(|_| "storage_unavailable")?;
        tx.execute("DELETE FROM attachments WHERE asset_id=?1", [asset])
            .map_err(|_| "storage_unavailable")?;
        tx.commit().map_err(|_| "storage_unavailable")?;
        Ok(())
    }

    /// Finish local removal after a durable marker, including after a restart.
    pub fn remove_purged_original(&self, asset: &str) -> Result<(), &'static str> {
        uuid(asset)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        let marked: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
                [asset],
                |r| r.get(0),
            )
            .map_err(|_| "storage_unavailable")?;
        if !marked {
            return Err("asset_not_purged");
        }
        let meta = asset_by_id(&c, asset)?;
        let path = self.object(&meta.sha256);
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("storage_unavailable"),
            Ok(m) if !m.file_type().is_file() => return Err("unsafe_location"),
            Ok(_) => {}
        }
        let (_, original) = self.open_verified(meta)?;
        drop(original);
        fs::remove_file(&path).map_err(|_| "storage_unavailable")?;
        File::open(path.parent().ok_or("unsafe_location")?)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| "storage_unavailable")?;
        Ok(())
    }

    pub fn open_visible(&self, asset: &str) -> Result<(Asset, File), &'static str> {
        uuid(asset)?;
        let _lock = self.lock()?;
        let c = self.db()?;
        let meta = asset_by_id(&c, asset)?;
        if trashed(&c, asset)? {
            return Err("asset_trashed");
        }
        self.open_verified(meta)
    }

    fn open_verified(&self, meta: Asset) -> Result<(Asset, File), &'static str> {
        for dir in [self.objects.clone(), self.objects.join("sha256")] {
            if !fs::symlink_metadata(dir)
                .map_err(|_| "storage_unavailable")?
                .file_type()
                .is_dir()
            {
                return Err("unsafe_location");
            }
        }
        let path = self.object(&meta.sha256);
        let parent = path.parent().ok_or("unsafe_location")?;
        if !fs::symlink_metadata(parent)
            .map_err(|_| "storage_unavailable")?
            .file_type()
            .is_dir()
        {
            return Err("unsafe_location");
        }
        let mut f = file(&path, false)?;
        let (hash, len) = digest(&f)?;
        if hash != meta.sha256 || len != meta.byte_length {
            return Err("integrity_error");
        }
        f.seek(SeekFrom::Start(0))
            .map_err(|_| "storage_unavailable")?;
        Ok((meta, f))
    }
}

fn trashed(c: &Connection, id: &str) -> Result<bool, &'static str> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM asset_trash WHERE asset_id=?1) OR EXISTS(SELECT 1 FROM asset_purged WHERE asset_id=?1)",
        [id],
        |r| r.get(0),
    )
    .map_err(|_| "storage_unavailable")
}

fn generation_by_id(c: &Connection, id: &str) -> Result<Option<Generation>, &'static str> {
    c.query_row("SELECT asset_id,prompt,prompt_source,provider,model,created_at,chat_id,message_id,lineage_id,version,version_of,version_how FROM media_generation WHERE asset_id=?1",[id],|r|Ok(Generation{asset_id:r.get(0)?,prompt:r.get(1)?,prompt_source:r.get(2)?,provider:r.get(3)?,model:r.get(4)?,created_at:r.get(5)?,chat_id:r.get(6)?,message_id:r.get(7)?,lineage_id:r.get(8)?,version:r.get(9)?,version_of:r.get(10)?,version_how:r.get(11)?})).optional().map_err(|_|"storage_unavailable")
}

fn asset_by_id(c: &Connection, id: &str) -> Result<Asset, &'static str> {
    let a: Asset = c
        .query_row(
            "SELECT id,hash,bytes,name,mime,kind FROM assets WHERE id=?1",
            [id],
            |r| {
                Ok(Asset {
                    id: r.get(0)?,
                    sha256: r.get(1)?,
                    byte_length: nonnegative(r, 2)?,
                    original_file_name: r.get(3)?,
                    media_type: r.get(4)?,
                    kind: r.get(5)?,
                })
            },
        )
        .optional()
        .map_err(|_| "storage_unavailable")?
        .ok_or("not_found")?;
    if a.byte_length > MAX_BYTES
        || a.sha256.len() != 64
        || !a
            .sha256
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err("integrity_error");
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Only(&'static str);
    impl ChatAccess for Only {
        fn may_access(&self, chat: &str) -> bool {
            self.0 == chat
        }
    }

    fn roots() -> (tempfile::TempDir, tempfile::TempDir) {
        (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
    }

    #[test]
    fn stale_incomplete_uploads_are_pruned_without_touching_completed_files() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let old = store.begin("old", "text/plain", 5, None).unwrap();
        let live = store.begin("live", "text/plain", 5, None).unwrap();
        let complete = store.begin("done", "text/plain", 5, None).unwrap();
        store.write_chunk(&complete.upload_id, 0, b"hello").unwrap();
        let asset = store.finish(&complete.upload_id).unwrap();
        let stage = File::open(store.stage(&old.upload_id)).unwrap();
        stage
            .set_times(std::fs::FileTimes::new().set_modified(
                std::time::SystemTime::now()
                    - std::time::Duration::from_secs(UPLOAD_STALE_SECONDS + 1),
            ))
            .unwrap();
        store.begin("new", "text/plain", 5, None).unwrap();
        assert_eq!(store.progress(&old.upload_id).unwrap_err(), "not_found");
        assert!(store.progress(&live.upload_id).is_ok());
        assert!(store.verify_asset(&asset.id).is_ok());
    }

    #[test]
    fn full_disk_error_has_the_shared_reason() {
        assert_eq!(
            write_error(std::io::Error::from_raw_os_error(libc::ENOSPC)),
            "disk_low"
        );
        assert!(!enough_space(19, 19, true, 10));
        assert!(enough_space(20, 20, true, 10));
        assert!(!enough_space(10, 9, false, 10));
        assert!(enough_space(10, 10, false, 10));
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        assert_eq!(
            store
                .begin("large", "application/octet-stream", MAX_BYTES, None)
                .unwrap_err(),
            "disk_low"
        );
        assert_eq!(
            fs::read_dir(data.path().join("upload-staging"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn two_uploads_are_isolated_and_out_of_order_or_missing_parts_fail() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let first = store
            .begin("a.bin", "application/octet-stream", 6, None)
            .unwrap();
        let second = store
            .begin("b.bin", "application/octet-stream", 3, None)
            .unwrap();
        assert_eq!(
            store.write_chunk(&first.upload_id, 3, b"def").unwrap_err(),
            "offset_mismatch"
        );
        store.write_chunk(&first.upload_id, 0, b"abc").unwrap();
        store.write_chunk(&second.upload_id, 0, b"xyz").unwrap();
        assert_eq!(
            store.write_chunk(&first.upload_id, 0, b"abc").unwrap_err(),
            "offset_mismatch"
        );
        assert_eq!(store.finish(&first.upload_id).unwrap_err(), "incomplete");
        let b = store.finish(&second.upload_id).unwrap();
        store.write_chunk(&first.upload_id, 3, b"def").unwrap();
        let a = store.finish(&first.upload_id).unwrap();
        assert_ne!(a.sha256, b.sha256);
    }

    #[test]
    fn concurrent_files_keep_their_own_data() {
        let (data, archive) = roots();
        Store::open(data.path(), archive.path()).unwrap();
        std::thread::scope(|scope| {
            let tasks: Vec<_> = (0..2u8)
                .map(|value| {
                    let data = data.path().to_owned();
                    let archive = archive.path().to_owned();
                    scope.spawn(move || {
                        let store = Store::open(&data, &archive).unwrap();
                        let upload = store
                            .begin("file.bin", "application/octet-stream", 4096, None)
                            .unwrap();
                        for offset in 0..4u64 {
                            store
                                .write_chunk(&upload.upload_id, offset * 1024, &[value; 1024])
                                .unwrap();
                        }
                        store.finish(&upload.upload_id).unwrap()
                    })
                })
                .collect();
            let assets: Vec<_> = tasks.into_iter().map(|task| task.join().unwrap()).collect();
            assert_ne!(assets[0].sha256, assets[1].sha256);
        });
    }

    #[test]
    #[ignore = "writes multi-gigabyte temporary fixture"]
    fn large_uploads_preserve_full_digest() {
        let chunk = vec![0x5a; MAX_CHUNK_BYTES];
        for mib in [10u64, 40, 200, 1024] {
            let (data, archive) = roots();
            let store = Store::open(data.path(), archive.path()).unwrap();
            let total = mib * 1024 * 1024;
            let mut expected = Sha256::new();
            let mut left = total;
            while left > 0 {
                let count = (left as usize).min(chunk.len());
                expected.update(&chunk[..count]);
                left -= count as u64;
            }
            let hash = hex::encode(expected.finalize());
            let upload = store
                .begin(
                    "recording.bin",
                    "application/octet-stream",
                    total,
                    Some(&hash),
                )
                .unwrap();
            let mut offset = 0;
            while offset < total {
                let count = ((total - offset) as usize).min(chunk.len());
                store
                    .write_chunk(&upload.upload_id, offset, &chunk[..count])
                    .unwrap();
                offset += count as u64;
            }
            let asset = store.finish(&upload.upload_id).unwrap();
            assert_eq!(asset.byte_length, total);
            assert_eq!(asset.sha256, hash);
        }
    }

    #[test]
    fn restored_purge_marker_keeps_name_but_prevents_resurrection() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let upload = store.begin("lesson.txt", "text/plain", 5, None).unwrap();
        store.write_chunk(&upload.upload_id, 0, b"hello").unwrap();
        let asset = store.finish(&upload.upload_id).unwrap();
        store
            .db()
            .unwrap()
            .execute("INSERT INTO asset_purged(asset_id) VALUES(?1)", [&asset.id])
            .unwrap();
        std::fs::remove_file(store.object(&asset.sha256)).unwrap();
        assert!(store.is_trashed(&asset.id).unwrap());
        assert_eq!(
            store
                .purged_asset(&asset.id)
                .unwrap()
                .unwrap()
                .original_file_name,
            "lesson.txt"
        );
        assert!(store.search_assets("lesson", 0, 10).unwrap().is_empty());
        assert_eq!(store.set_trashed(&asset.id, None), Err("asset_purged"));
        store
            .attach_restored(&Only("restored"), "restored", &asset.id)
            .unwrap();
        assert_eq!(
            store
                .open_attached(&Only("restored"), "restored", &asset.id)
                .unwrap_err(),
            "asset_trashed"
        );
        let again = store.begin("another.txt", "text/plain", 5, None).unwrap();
        store.write_chunk(&again.upload_id, 0, b"hello").unwrap();
        assert_eq!(store.finish(&again.upload_id).unwrap_err(), "asset_purged");
        assert!(!store.object(&asset.sha256).exists());
    }

    #[test]
    fn marked_original_removal_is_repeatable_and_checks_content() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let upload = store.begin("lesson.txt", "text/plain", 5, None).unwrap();
        store.write_chunk(&upload.upload_id, 0, b"hello").unwrap();
        let asset = store.finish(&upload.upload_id).unwrap();
        assert_eq!(
            store.remove_purged_original(&asset.id),
            Err("asset_not_purged")
        );
        assert_eq!(
            store.mark_purged(&asset.id, &asset.sha256),
            Err("asset_not_trashed")
        );
        store.set_trashed(&asset.id, Some("2026-11-22")).unwrap();
        assert_eq!(
            store.mark_purged(&asset.id, &"a".repeat(64)),
            Err("integrity_error")
        );
        store.mark_purged(&asset.id, &asset.sha256).unwrap();
        assert!(store.object(&asset.sha256).exists());
        store.remove_purged_original(&asset.id).unwrap();
        store.remove_purged_original(&asset.id).unwrap();
        assert!(!store.object(&asset.sha256).exists());
        assert!(store.purged_asset(&asset.id).unwrap().is_some());
    }

    #[test]
    fn trash_preserves_original_and_restored_chat_links() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let upload = store.begin("lesson.txt", "text/plain", 5, None).unwrap();
        store.write_chunk(&upload.upload_id, 0, b"hello").unwrap();
        let asset = store.finish(&upload.upload_id).unwrap();
        store.attach(&Only("chat"), "chat", &asset.id).unwrap();
        store
            .set_trashed(&asset.id, Some("2026-11-22T00:00:00Z"))
            .unwrap();
        assert!(store.is_trashed(&asset.id).unwrap());
        assert!(store.search_assets("lesson", 0, 10).unwrap().is_empty());
        assert_eq!(store.trashed_assets(0, 10).unwrap().len(), 1);
        assert_eq!(
            store
                .open_attached(&Only("chat"), "chat", &asset.id)
                .unwrap_err(),
            "asset_trashed"
        );
        assert!(store.verify_asset(&asset.id).is_ok());
        store
            .attach_restored(&Only("restored"), "restored", &asset.id)
            .unwrap();
        assert_eq!(
            store
                .open_attached(&Only("restored"), "restored", &asset.id)
                .unwrap_err(),
            "asset_trashed"
        );
        store.set_trashed(&asset.id, None).unwrap();
        let (_, mut file) = store
            .open_attached(&Only("restored"), "restored", &asset.id)
            .unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn generated_lineage_keeps_versions_and_user_upload_has_no_prompt() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let upload = |name: &str, bytes: &[u8]| {
            let p = store
                .begin(name, "image/png", bytes.len() as u64, None)
                .unwrap();
            store.write_chunk(&p.upload_id, 0, bytes).unwrap();
            store.finish(&p.upload_id).unwrap()
        };
        let first = upload(
            "one.png",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01first",
        );
        let second = upload(
            "two.png",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01second",
        );
        let user = upload("user.png", b"user fixture");
        let record = |id: &str, parent: Option<&str>| {
            store
                .record_generation(GenerationInput {
                    asset_id: id,
                    prompt: "draw",
                    prompt_source: "reported",
                    provider: "codex",
                    model: Some("fixture"),
                    created_at: "2026-11-22T00:00:00Z",
                    chat_id: "chat",
                    message_id: "message",
                    parent,
                    how: None,
                })
                .unwrap()
        };
        let one = record(&first.id, None);
        let two = record(&second.id, Some(&first.id));
        assert_eq!(one.lineage_id, two.lineage_id);
        assert_eq!(two.version, 2);
        assert_eq!(two.version_of.as_deref(), Some(first.id.as_str()));
        assert_eq!(two.version_how.as_deref(), Some("regenerated"));
        assert_eq!(store.versions(&one.lineage_id).unwrap().len(), 2);
        assert!(store.generation(&user.id).unwrap().is_none());
        let visible = store.list_media(Some("image"), None, 0, 20).unwrap();
        assert_eq!(visible.len(), 2);
        assert!(!visible.iter().any(|a| a.id == first.id));
    }

    #[test]
    fn duplicate_generated_upload_does_not_claim_user_owned_asset() {
        let (data, archive) = roots();
        let store = Store::open(data.path(), archive.path()).unwrap();
        let user = store.begin("user.png", "image/png", 4, None).unwrap();
        store.write_chunk(&user.upload_id, 0, b"same").unwrap();
        let asset = store.finish(&user.upload_id).unwrap();
        let generated = store.begin("output.png", "image/png", 4, None).unwrap();
        store
            .mark_generated_upload(&generated.upload_id, "chat", "message", "codex")
            .unwrap();
        store.write_chunk(&generated.upload_id, 0, b"same").unwrap();
        let (duplicate, created) = store.finish_with_created(&generated.upload_id).unwrap();
        assert_eq!(asset.id, duplicate.id);
        assert!(!created);
        assert!(!store
            .owns_generated_asset(&asset.id, "chat", "message", "codex")
            .unwrap());
        assert!(store.generation(&asset.id).unwrap().is_none());
    }

    #[test]
    fn phone_resume_attach_ai_read_download() {
        let (data, archive) = roots();
        let source = b"photo and transcript for class";
        let store = Store::open(data.path(), archive.path()).unwrap();
        let p = store
            .begin("수업.txt", "text/plain", source.len() as u64, None)
            .unwrap();
        assert_eq!(
            store
                .write_chunk(&p.upload_id, 0, &source[..7])
                .unwrap()
                .received,
            7
        );
        drop(store); // connection / process interrupted
        let store = Store::open(data.path(), archive.path()).unwrap();
        assert_eq!(store.progress(&p.upload_id).unwrap().received, 7);
        assert_eq!(
            store
                .write_chunk(&p.upload_id, 0, &source[..7])
                .unwrap_err(),
            "offset_mismatch"
        );
        store.write_chunk(&p.upload_id, 7, &source[7..]).unwrap();
        let a = store.finish(&p.upload_id).unwrap();
        let (date, changed) = store.asset_date(&a.id).unwrap();
        assert!(!changed);
        let date = date.unwrap();
        assert_eq!(date.1, "registered");
        assert!(chrono::DateTime::parse_from_rfc3339(&date.0).is_ok());
        {
            let c = store.db().unwrap();
            c.execute("DELETE FROM asset_dates WHERE asset_id=?1", [&a.id])
                .unwrap();
        }
        let (fallback, changed) = store.asset_date(&a.id).unwrap();
        assert!(changed);
        assert_eq!(fallback.unwrap().1, "file_modified");
        assert!(!store.asset_date(&a.id).unwrap().1);
        assert_eq!(a.kind, "text");
        assert_eq!(a.sha256, hex::encode(Sha256::digest(source)));
        assert_eq!(store.finish(&p.upload_id).unwrap().id, a.id);
        let object = archive
            .path()
            .join("objects/sha256")
            .join(&a.sha256[..2])
            .join(&a.sha256);
        assert_eq!(fs::read(&object).unwrap(), source);
        let (verified, _) = store.verify_asset(&a.id).unwrap();
        assert_eq!(verified.sha256, a.sha256);
        store.attach(&Only("chat-a"), "chat-a", &a.id).unwrap();
        assert!(store
            .open_attached(&Only("chat-b"), "chat-b", &a.id)
            .is_err());
        let (_, ai) = store
            .open_attached(&Only("chat-a"), "chat-a", &a.id)
            .unwrap();
        // A separate fake AI process reads only the host-authorized handle.
        let ai_result = std::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::from(ai))
            .output()
            .unwrap();
        assert!(ai_result.status.success());
        assert_eq!(ai_result.stdout, source);
        let (_, mut phone) = store
            .open_attached(&Only("chat-a"), "chat-a", &a.id)
            .unwrap();
        let mut downloaded = Vec::new();
        phone.read_to_end(&mut downloaded).unwrap();
        assert_eq!(downloaded, source);
        fs::set_permissions(&object, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&object, b"damaged").unwrap();
        assert_eq!(store.verify_asset(&a.id).unwrap_err(), "integrity_error");
    }

    #[test]
    fn invalid_paths_hash_and_corrupt_existing_never_overwrite() {
        let (data, archive) = roots();
        let s = Store::open(data.path(), archive.path()).unwrap();
        assert_eq!(
            s.begin("../outside", "text/plain", 1, None).unwrap_err(),
            "invalid_arguments"
        );
        assert_eq!(
            s.begin("file", "text/plain", MAX_BYTES + 1, None)
                .unwrap_err(),
            "invalid_arguments"
        );
        assert_eq!(s.progress("../x").unwrap_err(), "invalid_id");
        let p = s
            .begin("x", "application/octet-stream", 1, Some(&"0".repeat(64)))
            .unwrap();
        s.write_chunk(&p.upload_id, 0, b"a").unwrap();
        assert_eq!(s.finish(&p.upload_id).unwrap_err(), "integrity_error");
        let hash = hex::encode(Sha256::digest(b"a"));
        let dir = archive.path().join("objects/sha256").join(&hash[..2]);
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join(&hash), b"other").unwrap();
        let p = s.begin("x", "application/octet-stream", 1, None).unwrap();
        s.write_chunk(&p.upload_id, 0, b"a").unwrap();
        assert_eq!(s.finish(&p.upload_id).unwrap_err(), "integrity_error");
        assert_eq!(fs::read(dir.join(hash)).unwrap(), b"other");
    }

    #[test]
    fn unacknowledged_bytes_are_rolled_back_on_resume() {
        let (data, archive) = roots();
        let s = Store::open(data.path(), archive.path()).unwrap();
        let p = s.begin("x", "audio/wav", 6, None).unwrap();
        s.write_chunk(&p.upload_id, 0, b"abc").unwrap();
        let mut f = OpenOptions::new()
            .append(true)
            .open(s.stage(&p.upload_id))
            .unwrap();
        f.write_all(b"bad").unwrap();
        f.sync_all().unwrap();
        s.write_chunk(&p.upload_id, 3, b"def").unwrap();
        let a = s.finish(&p.upload_id).unwrap();
        assert_eq!(a.sha256, hex::encode(Sha256::digest(b"abcdef")));
    }

    #[test]
    fn symlink_stage_never_writes_outside() {
        use std::os::unix::fs::symlink;
        let (data, archive) = roots();
        let s = Store::open(data.path(), archive.path()).unwrap();
        let p = s.begin("x", "text/plain", 1, None).unwrap();
        let outside = data.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        fs::remove_file(s.stage(&p.upload_id)).unwrap();
        symlink(&outside, s.stage(&p.upload_id)).unwrap();
        assert!(s.write_chunk(&p.upload_id, 0, b"x").is_err());
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
    }

    #[test]
    fn large_recording_resumes_and_preserves_digest() {
        let (data, archive) = roots();
        let chunk = vec![0x5a; 4 * 1024 * 1024];
        let total = 64 * 1024 * 1024;
        let store = Store::open(data.path(), archive.path()).unwrap();
        let upload = store
            .begin("recording.wav", "audio/wav", total, None)
            .unwrap();
        let mut expected = Sha256::new();
        for part in 0..8u64 {
            store
                .write_chunk(&upload.upload_id, part * chunk.len() as u64, &chunk)
                .unwrap();
            expected.update(&chunk);
        }
        drop(store);
        let store = Store::open(data.path(), archive.path()).unwrap();
        assert_eq!(
            store.progress(&upload.upload_id).unwrap().received,
            total / 2
        );
        for part in 8..16u64 {
            store
                .write_chunk(&upload.upload_id, part * chunk.len() as u64, &chunk)
                .unwrap();
            expected.update(&chunk);
        }
        let asset = store.finish(&upload.upload_id).unwrap();
        assert_eq!(asset.byte_length, total);
        assert_eq!(asset.sha256, hex::encode(expected.finalize()));
    }
}
