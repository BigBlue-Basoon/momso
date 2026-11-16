use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tauri::Emitter;
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApprovalChoice {
    AllowOnce,
    AllowSession,
    Deny,
    Cancel,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeKind {
    Create,
    Edit,
    Overwrite,
    Delete,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ApprovalTarget {
    pub(crate) path: String,
    pub(crate) change: Option<ChangeKind>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub(crate) struct ApprovalRequest {
    pub(crate) request_id: String,
    pub(crate) chat_id: Option<String>,
    pub(crate) provider: String,
    pub(crate) kind: String,
    pub(crate) description: String,
    pub(crate) change: Option<ChangeKind>,
    pub(crate) targets: Vec<ApprovalTarget>,
    pub(crate) reason: Option<String>,
    pub(crate) options: Vec<ApprovalChoice>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalClosed {
    pub(crate) request_id: String,
    pub(crate) chat_id: Option<String>,
}
impl ApprovalRequest {
    pub(crate) fn closed(&self) -> ApprovalClosed {
        ApprovalClosed {
            request_id: self.request_id.clone(),
            chat_id: self.chat_id.clone(),
        }
    }
}

struct Pending {
    options: Vec<ApprovalChoice>,
    sender: oneshot::Sender<ApprovalChoice>,
}

#[derive(Clone, Default)]
pub(crate) struct ApprovalBroker {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl ApprovalBroker {
    async fn register(
        &self,
        id: String,
        options: Vec<ApprovalChoice>,
    ) -> Result<oneshot::Receiver<ApprovalChoice>, String> {
        let mut pending = self.pending.lock().await;
        if pending.contains_key(&id) || options.is_empty() {
            return Err("승인 요청을 등록하지 못했습니다".to_owned());
        }
        let (sender, receiver) = oneshot::channel();
        pending.insert(id, Pending { options, sender });
        Ok(receiver)
    }

    pub(crate) async fn respond(&self, id: &str, choice: ApprovalChoice) -> Result<(), String> {
        let mut pending = self.pending.lock().await;
        let request = pending
            .get(id)
            .ok_or_else(|| "승인 요청을 찾을 수 없습니다".to_owned())?;
        if !request.options.contains(&choice) {
            return Err("이 승인 요청에서 선택할 수 없는 응답입니다".to_owned());
        }
        let request = pending
            .remove(id)
            .ok_or_else(|| "승인 요청을 찾을 수 없습니다".to_owned())?;
        request
            .sender
            .send(choice)
            .map_err(|_| "승인 요청이 이미 끝났습니다".to_owned())
    }

    #[allow(dead_code)]
    pub(crate) async fn request(
        &self,
        app: &tauri::AppHandle,
        request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> Result<ApprovalChoice, String> {
        let id = request.request_id.clone();
        let closed = request.closed();
        let receiver = self.register(id.clone(), request.options.clone()).await?;
        if app.emit("agent-approval-request", &request).is_err() {
            self.pending.lock().await.remove(&id);
            return Err("승인 요청을 화면에 전달하지 못했습니다".to_owned());
        }
        tokio::select! {
            result = receiver => result.map_err(|_| "승인 요청이 이미 끝났습니다".to_owned()),
            _ = cancellation.cancelled() => {
                self.pending.lock().await.remove(&id);
                let _ = app.emit("agent-approval-closed", closed);
                Err("요청을 중단했습니다".to_owned())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_close_include_chat_id_or_null() {
        let mut request = ApprovalRequest {
            request_id: "req".into(),
            chat_id: Some("chat_123".into()),
            provider: "codex".into(),
            kind: "command".into(),
            description: "cmd".into(),
            change: None,
            targets: vec![],
            reason: None,
            options: vec![ApprovalChoice::Deny],
        };
        assert_eq!(
            serde_json::to_value(&request).unwrap()["chatId"],
            "chat_123"
        );
        assert_eq!(
            serde_json::to_value(request.closed()).unwrap()["chatId"],
            "chat_123"
        );
        request.chat_id = None;
        assert!(serde_json::to_value(&request).unwrap()["chatId"].is_null());
        assert!(serde_json::to_value(request.closed()).unwrap()["chatId"].is_null());
    }
    #[tokio::test]
    async fn allows_only_advertised_responses_and_delivers_once() {
        let broker = ApprovalBroker::default();
        let receiver = broker
            .register(
                "a".into(),
                vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny],
            )
            .await
            .unwrap();
        assert!(broker
            .respond("a", ApprovalChoice::AllowSession)
            .await
            .is_err());
        broker
            .respond("a", ApprovalChoice::AllowOnce)
            .await
            .unwrap();
        assert_eq!(receiver.await.unwrap(), ApprovalChoice::AllowOnce);
        assert!(broker.respond("a", ApprovalChoice::Deny).await.is_err());
    }

    #[tokio::test]
    async fn rejects_duplicate_and_empty_requests() {
        let broker = ApprovalBroker::default();
        let _receiver = broker
            .register("a".into(), vec![ApprovalChoice::Deny])
            .await
            .unwrap();
        assert!(broker
            .register("a".into(), vec![ApprovalChoice::Deny])
            .await
            .is_err());
        assert!(broker.register("b".into(), vec![]).await.is_err());
    }
}
