use crate::{chat, chat_handoff};
use serde_json::Value;
use std::path::Path;

#[derive(Default)]
pub(crate) struct Capture {
    text: String,
    pub(crate) model: Option<String>,
    effort: Option<String>,
    reported_reason: Option<chat::FinishReason>,
    pub(crate) provider_error: Option<Value>,
}
fn classified(kind: &'static str, code: &'static str, actions: &[&str]) -> Value {
    serde_json::json!({"kind":kind,"retryAt":null,"actions":actions,"providerCode":code})
}
pub(crate) fn codex_error(value: &Value) -> Option<Value> {
    match value.as_str()? {
        "usageLimitExceeded" | "sessionBudgetExceeded" => Some(classified(
            "usageLimit",
            if value == "usageLimitExceeded" {
                "usageLimitExceeded"
            } else {
                "sessionBudgetExceeded"
            },
            &["wait", "switchProvider"],
        )),
        "rateLimitExceeded" => Some(classified(
            "rateLimited",
            "rateLimitExceeded",
            &["wait", "retry", "switchProvider"],
        )),
        "unauthorized" => Some(classified(
            "authentication",
            "unauthorized",
            &["signIn", "switchProvider"],
        )),
        "modelNotFound" => Some(classified(
            "modelUnavailable",
            "modelNotFound",
            &["switchModel", "switchProvider"],
        )),
        "serverOverloaded" | "internalServerError" => Some(classified(
            "serviceUnavailable",
            if value == "serverOverloaded" {
                "serverOverloaded"
            } else {
                "internalServerError"
            },
            &["retry", "wait", "switchProvider"],
        )),
        "contextWindowExceeded" => Some(classified(
            "contextTooLong",
            "contextWindowExceeded",
            &["shorten", "switchProvider"],
        )),
        _ => None,
    }
}
pub(crate) fn claude_error(value: &Value) -> Option<Value> {
    if value["type"] == "rate_limit_event" && value["rate_limit_info"]["status"] == "rejected" {
        let info = &value["rate_limit_info"];
        let mut result = if info["errorCode"] == "credits_required" {
            classified(
                "usageLimit",
                "credits_required",
                &["checkBilling", "switchProvider"],
            )
        } else {
            classified(
                "rateLimited",
                "rate_limit_event",
                &["wait", "retry", "switchProvider"],
            )
        };
        if let Some(reset) = info["resetsAt"].as_i64().filter(|v| *v > 0) {
            result["retryAt"] = serde_json::json!(reset);
        }
        return Some(result);
    }
    if value["type"] != "assistant" {
        return None;
    }
    match value["error"].as_str()? {
        "authentication_failed" => Some(classified(
            "authentication",
            "authentication_failed",
            &["signIn", "switchProvider"],
        )),
        "rate_limit" => Some(classified(
            "rateLimited",
            "rate_limit",
            &["wait", "retry", "switchProvider"],
        )),
        "billing_error" => Some(classified(
            "billing",
            "billing_error",
            &["checkBilling", "switchProvider"],
        )),
        "model_not_found" => Some(classified(
            "modelUnavailable",
            "model_not_found",
            &["switchModel", "switchProvider"],
        )),
        "overloaded" | "server_error" => Some(classified(
            "serviceUnavailable",
            if value["error"] == "overloaded" {
                "overloaded"
            } else {
                "server_error"
            },
            &["retry", "wait", "switchProvider"],
        )),
        _ => None,
    }
}
pub(crate) fn incomplete_reply(payload: &Value) -> bool {
    matches!(
        payload.get("stopReason").and_then(Value::as_str),
        Some("max_tokens" | "max_turn_requests")
    )
}
pub(crate) fn provider_error_payload(chat_id: &str, error: &str) -> Value {
    match error {
        "__momso_fast_unavailable" => {
            serde_json::json!({"chatId":chat_id,"status":"fast_unavailable","reason":""})
        }
        "__momso_auth_failed" => {
            serde_json::json!({"chatId":chat_id,"status":"auth_failed","reason":"","error":{"kind":"authentication","retryAt":null,"actions":["signIn","switchProvider"],"providerCode":null}})
        }
        "__momso_tool_missing" => {
            serde_json::json!({"chatId":chat_id,"status":"unavailable","reason":"","error":classified("toolMissing", "tool_missing", &["install", "switchProvider"])})
        }
        "AI 연결이 끊겼습니다" => {
            serde_json::json!({"chatId":chat_id,"reason":"","error":classified("connectionClosed", "connection_closed", &["retry", "switchProvider"])})
        }
        "AI의 응답 시간이 초과되었습니다" => {
            serde_json::json!({"chatId":chat_id,"reason":"","error":classified("timeout", "timeout", &["retry", "switchProvider"])})
        }
        _ => {
            let _ = error;
            serde_json::json!({"chatId":chat_id,"reason":"","error":classified("unknown", "unknown", &[])})
        }
    }
}
impl Capture {
    pub(crate) fn partial_reason(&self, stopped: bool) -> chat::FinishReason {
        if stopped {
            chat::FinishReason::Cancelled
        } else {
            self.reported_reason.unwrap_or(chat::FinishReason::Error)
        }
    }
    pub(crate) fn observe(&mut self, name: &str, value: &Value) {
        match name {
            "chat-stream" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    self.text.push_str(delta);
                }
            }
            "momso-internal-observed-model" => self.model = value.as_str().map(str::to_owned),
            "momso-internal-observed-effort" => self.effort = value.as_str().map(str::to_owned),
            "momso-internal-provider-error" => {
                let mut next = value.clone();
                if let Some(previous) = &self.provider_error {
                    let both_limits = ["usageLimit", "rateLimited"]
                        .contains(&previous["kind"].as_str().unwrap_or(""))
                        && ["usageLimit", "rateLimited"]
                            .contains(&next["kind"].as_str().unwrap_or(""));
                    if both_limits
                        && previous["kind"] == "usageLimit"
                        && next["kind"] == "rateLimited"
                    {
                        next = previous.clone();
                    } else if both_limits
                        && next["retryAt"].is_null()
                        && !previous["retryAt"].is_null()
                    {
                        next["retryAt"] = previous["retryAt"].clone();
                    }
                }
                self.provider_error = Some(next);
            }
            "momso-internal-finish-reason" => {
                self.reported_reason = match value.as_str() {
                    Some("length_limit") => Some(chat::FinishReason::LengthLimit),
                    Some("turn_limit") => Some(chat::FinishReason::TurnLimit),
                    Some("refusal") => Some(chat::FinishReason::Refusal),
                    _ => None,
                }
            }
            _ => (),
        }
    }

    pub(crate) fn persist_partial(
        &self,
        db: &Path,
        chat_id: &str,
        provider: &str,
        stopped: bool,
        omitted: usize,
    ) -> Result<Option<String>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let reason = self.partial_reason(stopped);
        let message = chat::append_finished(
            db,
            chat_id,
            &self.text,
            provider.into(),
            self.model.clone(),
            self.effort.clone(),
            reason,
        )?;
        chat::set_delivery_metadata(db, chat_id, &message.id, None, omitted)?;
        chat_handoff::mark_delivered(db, chat_id, provider, &message.id)?;
        Ok(Some(message.id))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_errors_keep_only_safe_official_codes_and_report_reset() {
        use serde_json::json;
        let mut capture = super::Capture::default();
        let event = json!({"type":"rate_limit_event","rate_limit_info":{"status":"rejected","errorCode":"credits_required","resetsAt":1800000000},"secret":"fake-secret"});
        let info = super::claude_error(&event).unwrap();
        assert_eq!(info["kind"], "usageLimit");
        assert_eq!(info["retryAt"], 1800000000_i64);
        assert!(!info.to_string().contains("fake-secret"));
        capture.observe("momso-internal-provider-error", &info);
        let next = super::claude_error(&json!({"type":"assistant","error":"rate_limit"})).unwrap();
        capture.observe("momso-internal-provider-error", &next);
        assert_eq!(capture.provider_error.unwrap()["kind"], "usageLimit");
        assert!(super::claude_error(&json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1800000000}})).is_none());
        assert!(super::codex_error(&json!("fake-secret")).is_none());
    }
    #[test]
    fn provider_errors_expose_state_not_internal_sentinel() {
        for (error, status) in [
            ("__momso_auth_failed", "auth_failed"),
            ("__momso_fast_unavailable", "fast_unavailable"),
        ] {
            let payload = super::provider_error_payload("room", error);
            assert_eq!(payload["chatId"], "room");
            assert_eq!(payload["status"], status);
            assert_eq!(payload["reason"], "");
            assert!(!payload.to_string().contains(error));
        }
        let ordinary = super::provider_error_payload("room", "ordinary error");
        assert_eq!(ordinary["reason"], "");
        assert_eq!(ordinary["error"]["kind"], "unknown");
        assert!(!ordinary.to_string().contains("ordinary error"));
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn only_limits_mark_a_saved_reply_incomplete() {
        assert!(incomplete_reply(&json!({"stopReason":"max_tokens"})));
        assert!(incomplete_reply(&json!({"stopReason":"max_turn_requests"})));
        assert!(!incomplete_reply(&json!({"stopReason":"refusal"})));
        assert!(!incomplete_reply(&json!({})));
    }

    #[test]
    fn partial_replies_survive_reopen_for_all_providers() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db.sqlite");
        crate::storage::initialize(&db, &dir.path().join("backups"), "test").unwrap();
        let id = serde_json::to_value(chat::create(&db).unwrap()).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let empty = Capture::default();
        assert!(empty
            .persist_partial(&db, &id, "codex", true, 0)
            .unwrap()
            .is_none());
        for provider in ["codex", "claude", "gemini"] {
            for stopped in [true, false] {
                let mut capture = Capture::default();
                capture.observe("momso-internal-observed-model", &json!("fixture"));
                capture.observe("chat-stream", &json!({"delta":"first "}));
                capture.observe("chat-stream", &json!({"delta":"second"}));
                capture
                    .persist_partial(&db, &id, provider, stopped, 0)
                    .unwrap();
            }
        }
        let mut limited = Capture::default();
        limited.observe("chat-stream", &json!({"delta":"limited"}));
        limited.observe("momso-internal-finish-reason", &json!("length_limit"));
        limited
            .persist_partial(&db, &id, "codex", false, 0)
            .unwrap();
        limited.observe("momso-internal-finish-reason", &json!("turn_limit"));
        limited
            .persist_partial(&db, &id, "claude", false, 0)
            .unwrap();
        limited
            .persist_partial(&db, &id, "claude", true, 0)
            .unwrap();
        limited.observe("momso-internal-finish-reason", &json!("refusal"));
        limited
            .persist_partial(&db, &id, "claude", false, 0)
            .unwrap();
        let detail = serde_json::to_value(chat::read(&db, &id).unwrap()).unwrap();
        let messages = detail["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 10);
        for (index, message) in messages.iter().take(6).enumerate() {
            assert_eq!(message["text"], "first second");
            assert_eq!(
                message["provider"],
                ["codex", "claude", "gemini"][index / 2]
            );
            assert_eq!(message["model"], "fixture");
            assert_eq!(
                message["finishReason"],
                if index % 2 == 0 { "cancelled" } else { "error" }
            );
        }
        assert_eq!(messages[6]["finishReason"], "length_limit");
        assert_eq!(messages[7]["finishReason"], "turn_limit");
        assert_eq!(messages[8]["finishReason"], "cancelled");
        assert_eq!(messages[9]["finishReason"], "refusal");
    }
}
