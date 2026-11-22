//! Portable pieces of the Codex app-server protocol shared by desktop and server.
use serde_json::{json, Value};

pub(crate) fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"id": id, "method": method, "params": params})
}

pub(crate) fn text_input(text: &str) -> Value {
    json!([{"type":"text","text":text}])
}

pub(crate) fn message_delta(value: &Value) -> Option<&str> {
    if value.get("method")?.as_str()? != "item/agentMessage/delta" {
        return None;
    }
    value.pointer("/params/delta")?.as_str()
}

pub(crate) fn completed_turn(value: &Value) -> Option<bool> {
    if value.get("method")?.as_str()? != "turn/completed" {
        return None;
    }
    value
        .pointer("/params/turn/status")?
        .as_str()
        .map(|v| v == "completed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_protocol_recognizes_text_and_outcomes() {
        assert_eq!(
            message_delta(&json!({"method":"item/agentMessage/delta","params":{"delta":"안녕"}})),
            Some("안녕")
        );
        assert_eq!(
            completed_turn(
                &json!({"method":"turn/completed","params":{"turn":{"status":"failed"}}})
            ),
            Some(false)
        );
        assert_eq!(
            completed_turn(
                &json!({"method":"turn/completed","params":{"turn":{"status":"completed"}}})
            ),
            Some(true)
        );
        assert_eq!(completed_turn(&json!({"method":"other"})), None);
    }
}
