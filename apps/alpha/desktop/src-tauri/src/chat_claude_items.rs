#[cfg(test)]
use crate::archive_receipt;
use crate::archive_zet;
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(crate) struct Items {
    pending: HashMap<String, (String, PathBuf)>,
    read: BTreeSet<String>,
    wrote: BTreeSet<String>,
}
impl Items {
    pub(crate) fn written(&self) -> Vec<String> {
        self.wrote.iter().cloned().collect()
    }
    pub(crate) fn read(&self) -> Vec<String> {
        self.read.iter().cloned().collect()
    }
    pub(crate) fn observe(&mut self, archive: &Path, event: &Value) {
        let Some(content) = event.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in content {
            if event["type"] == "assistant" && block["type"] == "tool_use" {
                let (Some(id), Some(tool), Some(path)) = (
                    block["id"].as_str(),
                    block["name"].as_str(),
                    block.pointer("/input/file_path").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if matches!(tool, "Read" | "Write" | "Edit" | "MultiEdit") {
                    let path = PathBuf::from(path);
                    self.pending.insert(
                        id.into(),
                        (
                            tool.into(),
                            if path.is_absolute() {
                                path
                            } else {
                                archive.join(path)
                            },
                        ),
                    );
                }
            } else if event["type"] == "user" && block["type"] == "tool_result" {
                let Some(id) = block["tool_use_id"].as_str() else {
                    continue;
                };
                let Some((tool, path)) = self.pending.remove(id) else {
                    continue;
                };
                if block["is_error"].as_bool() == Some(true) {
                    continue;
                }
                if let Some(reference) = reference(archive, &path) {
                    if tool == "Read" {
                        self.read.insert(reference);
                    } else if archive_zet::valid_zet_id(&reference)
                        || archive_zet::valid_asset_id(&reference)
                    {
                        self.wrote.insert(reference);
                    }
                }
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn save(
        self,
        archive: &Path,
        db: &Path,
        chat_id: &str,
        model: Option<&str>,
    ) -> Result<bool, String> {
        if self.read.is_empty() && self.wrote.is_empty() {
            return Ok(false);
        }
        let c = rusqlite::Connection::open(db)
            .map_err(|_| "작업 내역을 저장하지 못했습니다".to_string())?;
        let prompt: String = c.query_row("SELECT text FROM chat_messages WHERE chat_id=?1 AND role='user' ORDER BY rowid DESC LIMIT 1", [chat_id], |r| r.get(0)).map_err(|_| "작업 내역을 저장하지 못했습니다".to_string())?;
        let changes = format!(
            "읽은 자료: {}\n고친 문서: {}",
            self.read.iter().cloned().collect::<Vec<_>>().join(", "),
            self.wrote.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        archive_receipt::create(
            archive,
            db,
            archive_receipt::ReceiptCreate {
                tool: "claude".into(),
                model: model.unwrap_or("").into(),
                chat: Some(chat_id.into()),
                read: self.read.into_iter().collect(),
                wrote: self.wrote.into_iter().collect(),
                prompt,
                changes,
            },
        )?;
        Ok(true)
    }
}
fn reference(archive: &Path, path: &Path) -> Option<String> {
    let path = path.canonicalize().ok()?;
    let root = archive.canonicalize().ok()?;
    let rel = path.strip_prefix(root).ok()?;
    let parts = rel.iter().map(|s| s.to_str()).collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        ["zettels", file] => {
            let id = file.strip_suffix(".md")?;
            archive_zet::valid_zet_id(id).then(|| id.to_string())
        }
        ["objects", "sha256", prefix, hash] if hash.starts_with(prefix) && prefix.len() == 2 => {
            let id = format!("sha256:{hash}");
            archive_zet::valid_asset_id(&id).then_some(id)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn records_only_successful_recognized_file_tools() {
        let d = tempfile::tempdir().unwrap();
        let zets = d.path().join("zettels");
        std::fs::create_dir(&zets).unwrap();
        let path = zets.join("zet_20261116_030000.md");
        std::fs::write(&path, "test").unwrap();
        let mut items = Items::default();
        items.observe(d.path(), &json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Read","input":{"file_path":path}}]}}));
        items.observe(d.path(), &json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","is_error":true}]}}));
        assert!(items.read.is_empty());
        items.observe(d.path(), &json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"b","name":"Edit","input":{"file_path":path}}]}}));
        items.observe(d.path(), &json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b"}]}}));
        assert!(items.wrote.contains("zet_20261116_030000"));
        assert!(reference(d.path(), &d.path().join("../other.md")).is_none());
    }
    #[test]
    fn successful_edit_creates_linked_receipt() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("archive");
        std::fs::create_dir_all(root.join("zettels")).unwrap();
        std::fs::create_dir_all(root.join("receipts")).unwrap();
        let db = d.path().join("test.sqlite");
        crate::storage::initialize(&db, &d.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        crate::chat::append(&db, &id, crate::chat::Role::User, "고쳐 주세요", None, None).unwrap();
        let file = root.join("zettels/zet_20261116_030000.md");
        std::fs::write(&file, "---\nid: zet_20261116_030000\ntitle: 수업\nsummary: 요약\nkind: note\nstatus: draft\ncreated_at: '2026-11-16T03:00:00+09:00'\nupdated_at: '2026-11-16T03:00:00+09:00'\nassets: []\nedges: []\n---\n본문\n").unwrap();
        let mut items = Items::default();
        items.observe(&root, &json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"edit","name":"Edit","input":{"file_path":file}}]}}));
        items.observe(&root, &json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"edit"}]}}));
        assert!(items.save(&root, &db, &id, Some("haiku")).unwrap());
        let linked = serde_json::to_value(
            archive_receipt::chats_for_reference(&root, &db, "zet_20261116_030000").unwrap(),
        )
        .unwrap();
        assert_eq!(linked[0]["id"], id);
        assert_eq!(linked[0]["by"]["provider"], "claude");
        let listed =
            serde_json::to_value(crate::chat_items::list(&root, &db, &id).unwrap()).unwrap();
        assert_eq!(listed["documents"][0]["id"], "zet_20261116_030000");
        assert_eq!(listed["documents"][0]["by"]["provider"], "claude");
        assert_eq!(listed["documents"][0]["by"]["model"], "haiku");
    }
}
