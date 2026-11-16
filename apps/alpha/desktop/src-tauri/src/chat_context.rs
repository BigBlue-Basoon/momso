use chrono::Local;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::{path::Path, time::Duration};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Context {
    state: &'static str,
    used_tokens: Option<i64>,
    limit_tokens: Option<i64>,
    used_percent: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    updated_at: Option<String>,
}
impl Context {
    pub(crate) fn measurement_differs(&self, other: &Self) -> bool {
        self.used_tokens != other.used_tokens
            || self.limit_tokens != other.limit_tokens
            || self.provider != other.provider
            || self.model != other.model
    }
}
fn db(path: &Path) -> Result<Connection, String> {
    let c = Connection::open(path).map_err(|_| "채팅방 정보를 읽지 못했습니다".to_string())?;
    c.busy_timeout(Duration::from_secs(5))
        .map_err(|_| "채팅방 정보를 읽지 못했습니다".to_string())?;
    Ok(c)
}
fn blank() -> Context {
    Context {
        state: "unavailable",
        used_tokens: None,
        limit_tokens: None,
        used_percent: None,
        provider: None,
        model: None,
        updated_at: None,
    }
}
pub(crate) fn switch_provider(
    path: &Path,
    chat_id: &str,
    provider: &str,
) -> Result<Option<Context>, String> {
    let old = get(path, chat_id)?;
    if old.provider.as_deref() == Some(provider) {
        return Ok(None);
    }
    let next = Context {
        state: "unavailable",
        used_tokens: None,
        limit_tokens: None,
        used_percent: None,
        provider: Some(provider.to_string()),
        model: None,
        updated_at: Some(Local::now().to_rfc3339()),
    };
    db(path)?.execute("INSERT INTO chat_context_state(chat_id,provider,model,used_tokens,limit_tokens,updated_at) VALUES(?1,?2,NULL,NULL,NULL,?3) ON CONFLICT(chat_id) DO UPDATE SET provider=excluded.provider,model=NULL,used_tokens=NULL,limit_tokens=NULL,updated_at=excluded.updated_at", params![chat_id,next.provider,next.updated_at]).map_err(|_| "채팅방 정보를 저장하지 못했습니다".to_string())?;
    Ok(Some(next))
}
fn percent(used: Option<i64>, limit: Option<i64>) -> Option<f64> {
    match (used, limit) {
        (Some(u), Some(l)) if l > 0 => Some((u as f64) * 100.0 / (l as f64)),
        _ => None,
    }
}
pub(crate) fn get(path: &Path, chat_id: &str) -> Result<Context, String> {
    let c = db(path)?;
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chats WHERE id=?1)",
            [chat_id],
            |r| r.get(0),
        )
        .map_err(|_| "채팅방 정보를 읽지 못했습니다".to_string())?;
    if !exists {
        return Err("채팅방을 찾을 수 없습니다".into());
    }
    let value=c.query_row("SELECT provider,model,used_tokens,limit_tokens,updated_at FROM chat_context_state WHERE chat_id=?1",[chat_id],|r| {
        let used: Option<i64> = r.get(2)?; let limit: Option<i64> = r.get(3)?;
        Ok(Context{state:if used.is_some(){"ok"}else{"unavailable"},provider:r.get(0)?,model:r.get(1)?,used_tokens:used,limit_tokens:limit,used_percent:percent(used,limit),updated_at:r.get(4)?})
    }).optional().map_err(|_|"채팅방 정보를 읽지 못했습니다".to_string())?;
    Ok(value.unwrap_or_else(blank))
}
pub(crate) fn update(
    path: &Path,
    chat_id: &str,
    model: Option<&str>,
    params: &Value,
) -> Result<Option<Context>, String> {
    let used = params
        .pointer("/tokenUsage/last/totalTokens")
        .and_then(Value::as_i64)
        .filter(|v| *v >= 0);
    let limit = params
        .pointer("/tokenUsage/modelContextWindow")
        .and_then(Value::as_i64)
        .filter(|v| *v > 0);
    let Some(used) = used else { return Ok(None) };
    update_measurement(path, chat_id, "codex", model, used, limit)
}
pub(crate) fn update_measurement(
    path: &Path,
    chat_id: &str,
    provider: &str,
    model: Option<&str>,
    used: i64,
    limit: Option<i64>,
) -> Result<Option<Context>, String> {
    if used < 0 || limit.is_some_and(|value| value <= 0) {
        return Ok(None);
    }
    let old = get(path, chat_id)?;
    if let (Some(model), Some(limit)) = (model, limit) {
        record_model_limit(path, chat_id, provider, model, used, limit)?;
    }
    let model = model.map(str::to_owned).or_else(|| {
        if old.provider.as_deref() == Some(provider) {
            old.model.clone()
        } else {
            None
        }
    });
    let next = Context {
        state: "ok",
        used_tokens: Some(used),
        limit_tokens: limit,
        used_percent: percent(Some(used), limit),
        provider: Some(provider.into()),
        model,
        updated_at: Some(Local::now().to_rfc3339()),
    };
    if !old.measurement_differs(&next) {
        return Ok(None);
    }
    db(path)?.execute("INSERT INTO chat_context_state(chat_id,provider,model,used_tokens,limit_tokens,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(chat_id) DO UPDATE SET provider=excluded.provider,model=excluded.model,used_tokens=excluded.used_tokens,limit_tokens=excluded.limit_tokens,updated_at=excluded.updated_at",params![chat_id,next.provider,next.model,next.used_tokens,next.limit_tokens,next.updated_at]).map_err(|_|"채팅방 정보를 저장하지 못했습니다".to_string())?;
    Ok(Some(next))
}

pub(crate) fn record_model_limit(
    path: &Path,
    chat_id: &str,
    provider: &str,
    model: &str,
    used: i64,
    limit: i64,
) -> Result<(), String> {
    if !matches!(provider, "claude" | "gemini")
        || model.trim().is_empty()
        || model == "default"
        || used < 0
        || limit <= 0
    {
        return Ok(());
    }
    db(path)?.execute("INSERT INTO provider_context_limits(chat_id,provider,model,limit_tokens,used_tokens,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(chat_id,provider,model) DO UPDATE SET limit_tokens=excluded.limit_tokens,used_tokens=excluded.used_tokens,updated_at=excluded.updated_at", params![chat_id,provider,model,limit,used,Local::now().to_rfc3339()]).map_err(|_| "채팅방 정보를 저장하지 못했습니다".to_string())?;
    Ok(())
}

pub(crate) fn model_limit(
    path: &Path,
    chat_id: &str,
    provider: &str,
    model: Option<&str>,
) -> Result<Option<(i64, i64)>, String> {
    let Some(model) = model.filter(|value| !value.trim().is_empty() && *value != "default") else {
        return Ok(None);
    };
    if !matches!(provider, "claude" | "gemini") {
        return Ok(None);
    }
    db(path)?.query_row("SELECT limit_tokens,used_tokens FROM provider_context_limits WHERE chat_id=?1 AND provider=?2 AND model=?3", params![chat_id,provider,model], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_| "채팅방 정보를 읽지 못했습니다".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("momso.db");
        crate::storage::initialize(&path, &temp.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(crate::chat::create(&path).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        (temp, path, id)
    }
    #[test]
    fn switching_provider_clears_previous_context() {
        let (_t, p, id) = fixture();
        update(
            &p,
            &id,
            Some("codex-model"),
            &json!({"tokenUsage":{"last":{"totalTokens":120},"modelContextWindow":1000}}),
        )
        .unwrap();
        let changed = switch_provider(&p, &id, "claude").unwrap().unwrap();
        assert_eq!(changed.state, "unavailable");
        assert_eq!(changed.provider.as_deref(), Some("claude"));
        assert_eq!(changed.used_tokens, None);
        assert_eq!(get(&p, &id).unwrap().provider.as_deref(), Some("claude"));
        assert!(switch_provider(&p, &id, "claude").unwrap().is_none());
        assert_eq!(
            switch_provider(&p, &id, "codex").unwrap().unwrap().state,
            "unavailable"
        );
    }
    #[test]
    fn provider_model_measurements_survive_switch_and_are_scoped() {
        let (_t, p, id) = fixture();
        update_measurement(&p, &id, "claude", Some("claude-fake"), 10, Some(5000)).unwrap();
        switch_provider(&p, &id, "gemini").unwrap();
        update_measurement(&p, &id, "gemini", Some("gemini-fake"), 20, Some(6000)).unwrap();
        assert_eq!(
            model_limit(&p, &id, "claude", Some("claude-fake")).unwrap(),
            Some((5000, 10))
        );
        assert_eq!(
            model_limit(&p, &id, "gemini", Some("gemini-fake")).unwrap(),
            Some((6000, 20))
        );
        assert_eq!(
            model_limit(&p, &id, "codex", Some("claude-fake")).unwrap(),
            None
        );
        assert_eq!(
            model_limit(&p, &id, "gemini", Some("claude-fake")).unwrap(),
            None
        );
    }
    #[test]
    fn unavailable_and_missing_chat() {
        let (_t, p, id) = fixture();
        assert_eq!(get(&p, &id).unwrap().state, "unavailable");
        assert!(get(&p, "missing").is_err());
    }
    #[test]
    fn last_context_is_saved_not_cumulative_and_emits_only_changes() {
        let (_t, p, id) = fixture();
        let input = json!({"tokenUsage":{"last":{"totalTokens":120},"total":{"totalTokens":800},"modelContextWindow":1000}});
        let first = update(&p, &id, Some("model"), &input).unwrap().unwrap();
        assert_eq!(first.used_tokens, Some(120));
        assert_eq!(first.used_percent, Some(12.0));
        assert!(update(&p, &id, Some("model"), &input).unwrap().is_none());
        let next = update(
            &p,
            &id,
            Some("model"),
            &json!({"tokenUsage":{"last":{"totalTokens":200},"modelContextWindow":1000}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next.used_percent, Some(20.0));
        assert!(first.measurement_differs(&next));
        assert_eq!(get(&p, &id).unwrap().used_tokens, Some(200));
    }
    #[test]
    fn missing_values_are_not_guessed() {
        let (_t, p, id) = fixture();
        assert!(update(&p, &id, None, &json!({"tokenUsage":{}}))
            .unwrap()
            .is_none());
        let value = update(
            &p,
            &id,
            None,
            &json!({"tokenUsage":{"last":{"totalTokens":7},"modelContextWindow":null}}),
        )
        .unwrap()
        .unwrap();
        assert!(
            value.limit_tokens.is_none() && value.used_percent.is_none() && value.model.is_none()
        );
    }
}
