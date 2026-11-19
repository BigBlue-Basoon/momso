use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tauri::{Emitter, Manager};
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetupApproval {
    pub(crate) action: &'static str,
    pub(crate) service: String,
    pub(crate) account: String,
    pub(crate) region: Option<String>,
    pub(crate) resource_name: String,
    pub(crate) visibility: &'static str,
    pub(crate) cost: &'static str,
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
    pub(crate) command: Option<String>,
    pub(crate) sandbox_escalation: Option<bool>,
    pub(crate) change: Option<ChangeKind>,
    pub(crate) targets: Vec<ApprovalTarget>,
    pub(crate) reason: Option<String>,
    pub(crate) options: Vec<ApprovalChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) setup: Option<SetupApproval>,
}

pub(crate) fn setup_request(
    chat_id: &str,
    ai_provider: &str,
    setup: SetupApproval,
) -> ApprovalRequest {
    ApprovalRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: Some(chat_id.to_owned()),
        provider: ai_provider.to_owned(),
        kind: "setup_change".into(),
        description: String::new(),
        command: None,
        sandbox_escalation: None,
        change: Some(ChangeKind::Create),
        targets: vec![ApprovalTarget {
            path: setup.resource_name.clone(),
            change: Some(ChangeKind::Create),
        }],
        reason: None,
        options: vec![
            ApprovalChoice::AllowOnce,
            ApprovalChoice::Deny,
            ApprovalChoice::Cancel,
        ],
        setup: Some(setup),
    }
}

pub(crate) fn browser_request(
    chat_id: &str,
    provider: &str,
    site: &str,
    action: &str,
    effect: &str,
    element_name: &str,
) -> ApprovalRequest {
    ApprovalRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: Some(chat_id.to_owned()),
        provider: provider.to_owned(),
        kind: "browser_action".into(),
        description: format!("{action} · {element_name}"),
        command: None,
        sandbox_escalation: None,
        change: Some(ChangeKind::Edit),
        targets: vec![ApprovalTarget {
            path: site.to_owned(),
            change: Some(ChangeKind::Edit),
        }],
        reason: Some(effect.to_owned()),
        options: vec![
            ApprovalChoice::AllowOnce,
            ApprovalChoice::Deny,
            ApprovalChoice::Cancel,
        ],
        setup: None,
    }
}

pub(crate) fn browser_scope_request(
    chat_id: &str,
    provider: &str,
    site: &str,
    skill: &str,
    action: &str,
    description: &str,
) -> ApprovalRequest {
    let mut request = browser_request(chat_id, provider, site, skill, "scope", action);
    request.kind = "browser_action_scope".into();
    let label = crate::skills::BUNDLED_SKILLS
        .iter()
        .find(|(name, _)| *name == skill)
        .and_then(|(_, source)| crate::skills::parse(skill, source).ok())
        .map(|(head, _)| head.title)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| skill.to_owned());
    request.description = format!("{label} · {action} · {description}");
    request
}

pub(crate) fn dictionary_request(
    chat_id: &str,
    provider: &str,
    description: String,
) -> ApprovalRequest {
    ApprovalRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: Some(chat_id.to_owned()),
        provider: provider.to_owned(),
        kind: "dictionary_change".into(),
        description,
        command: None,
        sandbox_escalation: None,
        change: Some(ChangeKind::Edit),
        targets: vec![ApprovalTarget {
            path: "dictionary/my-terms.json".into(),
            change: Some(ChangeKind::Edit),
        }],
        reason: None,
        options: vec![
            ApprovalChoice::AllowOnce,
            ApprovalChoice::Deny,
            ApprovalChoice::Cancel,
        ],
        setup: None,
    }
}

pub(crate) fn document_request(
    chat_id: &str,
    provider: &str,
    title: &str,
    id: Option<&str>,
) -> ApprovalRequest {
    let change = if id.is_some() {
        ChangeKind::Edit
    } else {
        ChangeKind::Create
    };
    ApprovalRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: Some(chat_id.to_owned()),
        provider: provider.to_owned(),
        kind: "file_change".into(),
        description: title.to_owned(),
        command: None,
        sandbox_escalation: None,
        change: Some(change),
        targets: vec![ApprovalTarget {
            path: id.map_or_else(|| "zettels/".to_owned(), |id| format!("zettels/{id}.md")),
            change: Some(change),
        }],
        reason: None,
        options: vec![
            ApprovalChoice::AllowOnce,
            ApprovalChoice::Deny,
            ApprovalChoice::Cancel,
        ],
        setup: None,
    }
}

pub(crate) fn dictionary_delete_request(
    chat_id: &str,
    provider: &str,
    description: String,
) -> ApprovalRequest {
    let mut request = dictionary_request(chat_id, provider, description);
    request.change = Some(ChangeKind::Delete);
    request.targets[0].change = Some(ChangeKind::Delete);
    request
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApprovalClosed {
    pub(crate) request_id: String,
    pub(crate) chat_id: Option<String>,
    pub(crate) reason: &'static str,
}
impl ApprovalRequest {
    pub(crate) fn redact_sensitive(&mut self, known: &[&str]) {
        let clean = |value: &mut String| {
            *value = crate::chat_archive_redact::redact_with_known(value, known).text;
        };
        clean(&mut self.description);
        if let Some(command) = &mut self.command {
            clean(command);
        }
        for target in &mut self.targets {
            clean(&mut target.path);
        }
        if let Some(reason) = &mut self.reason {
            clean(reason);
        }
        if let Some(setup) = &mut self.setup {
            clean(&mut setup.service);
            clean(&mut setup.account);
            clean(&mut setup.resource_name);
            if let Some(region) = &mut setup.region {
                clean(region);
            }
        }
    }

    pub(crate) fn closed(&self) -> ApprovalClosed {
        ApprovalClosed {
            request_id: self.request_id.clone(),
            chat_id: self.chat_id.clone(),
            reason: "provider_cancelled",
        }
    }
}

struct Pending {
    request: ApprovalRequest,
    sender: oneshot::Sender<ApprovalChoice>,
    deadline: tokio::time::Instant,
    cancellation: CancellationToken,
}

#[derive(Clone, Default)]
pub(crate) struct ApprovalBroker {
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}

impl ApprovalBroker {
    async fn register(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
        timeout: std::time::Duration,
    ) -> Result<oneshot::Receiver<ApprovalChoice>, String> {
        let mut pending = self.pending.lock().await;
        if pending.contains_key(&request.request_id) || request.options.is_empty() {
            return Err("승인 요청을 등록하지 못했습니다".to_owned());
        }
        let (sender, receiver) = oneshot::channel();
        pending.insert(
            request.request_id.clone(),
            Pending {
                request,
                sender,
                deadline: tokio::time::Instant::now() + timeout,
                cancellation,
            },
        );
        Ok(receiver)
    }

    pub(crate) async fn pending(&self, chat_id: Option<&str>) -> Vec<ApprovalRequest> {
        let pending = self.pending.lock().await;
        let mut requests: Vec<_> = pending
            .values()
            .filter(|entry| {
                !entry.cancellation.is_cancelled()
                    && !entry.sender.is_closed()
                    && tokio::time::Instant::now() < entry.deadline
                    && chat_id.is_none_or(|id| entry.request.chat_id.as_deref() == Some(id))
            })
            .map(|entry| entry.request.clone())
            .collect();
        requests.sort_by(|a, b| a.request_id.cmp(&b.request_id));
        requests
    }

    pub(crate) async fn respond(&self, id: &str, choice: ApprovalChoice) -> Result<(), String> {
        let mut pending = self.pending.lock().await;
        let request = pending
            .get(id)
            .ok_or_else(|| "승인 요청을 찾을 수 없습니다".to_owned())?;
        if request.cancellation.is_cancelled()
            || request.sender.is_closed()
            || tokio::time::Instant::now() >= request.deadline
        {
            return Err("승인 요청을 찾을 수 없습니다".to_owned());
        }
        if !request.request.options.contains(&choice) {
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
        mut request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> Result<ApprovalChoice, String> {
        let state = app.try_state::<crate::AppState>().ok_or("unavailable")?;
        let data_root = state.paths.data_root.clone();
        let database = state.paths.database_path.clone();
        let known = tokio::task::spawn_blocking(move || {
            crate::chat_archive_store::known_values(&data_root, &database)
        })
        .await
        .map_err(|_| "unavailable")?
        .map_err(|_| "unavailable")?;
        let refs: Vec<&str> = known.iter().map(String::as_str).collect();
        request.redact_sensitive(&refs);
        self.request_with(
            request,
            cancellation,
            |request| app.emit("agent-approval-request", request).is_ok(),
            |closed| {
                let _ = app.emit("agent-approval-closed", closed);
            },
        )
        .await
    }

    pub(crate) async fn request_with(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
        emit: impl FnOnce(&ApprovalRequest) -> bool,
        close: impl FnOnce(ApprovalClosed),
    ) -> Result<ApprovalChoice, String> {
        self.request_with_timeout(
            request,
            cancellation,
            emit,
            close,
            std::time::Duration::from_secs(300),
        )
        .await
    }

    async fn request_with_timeout(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
        emit: impl FnOnce(&ApprovalRequest) -> bool,
        close: impl FnOnce(ApprovalClosed),
        timeout: std::time::Duration,
    ) -> Result<ApprovalChoice, String> {
        let id = request.request_id.clone();
        let mut closed = request.closed();
        let receiver = self
            .register(request.clone(), cancellation.clone(), timeout)
            .await?;
        if !emit(&request) {
            self.pending.lock().await.remove(&id);
            return Err("승인 요청을 화면에 전달하지 못했습니다".to_owned());
        }
        tokio::select! {
            result = receiver => {
                closed.reason = if result.is_ok() { "responded" } else { "provider_cancelled" };
                close(closed);
                result.map_err(|_| "승인 요청이 이미 끝났습니다".to_owned())
            },
            _ = cancellation.cancelled() => {
                self.pending.lock().await.remove(&id);
                close(closed);
                Err("요청을 중단했습니다".to_owned())
            },
            _ = tokio::time::sleep(timeout) => {
                self.pending.lock().await.remove(&id);
                closed.reason = "timeout";
                close(closed);
                Err("timeout".to_owned())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_scope_displays_bundled_skill_title() {
        for (name, title) in [
            ("naver-smartplace", "네이버 스마트플레이스 운영"),
            ("soomgo-operations", "숨고 서비스 운영"),
            ("sssd-host-operations", "솜씨당 수업 운영"),
            ("frip-host-operations", "프립 수업 운영"),
            ("onstudio-operations", "온스튜디오 운영"),
            ("studiomate-operations", "스튜디오메이트 운영"),
        ] {
            let request = browser_scope_request(
                "chat",
                "codex",
                "https://example.test",
                name,
                "사이트 범위",
                "패널에서 사이트를 엽니다",
            );
            assert!(request.description.starts_with(title));
            assert!(!request.description.contains(name));
        }
    }

    #[tokio::test]
    async fn closed_event_distinguishes_timeout_answer_and_cancellation() {
        for (mode, expected) in [(0, "timeout"), (1, "responded"), (2, "provider_cancelled")] {
            let broker = ApprovalBroker::default();
            let responder = broker.clone();
            let token = CancellationToken::new();
            let cancel = token.clone();
            let request = browser_request(
                "chat",
                "codex",
                "https://example.test",
                "click",
                "publish",
                "send",
            );
            let (tx, rx) = oneshot::channel();
            let result = broker
                .request_with_timeout(
                    request,
                    token,
                    move |card| {
                        let id = card.request_id.clone();
                        tokio::spawn(async move {
                            if mode == 1 {
                                responder
                                    .respond(&id, ApprovalChoice::AllowOnce)
                                    .await
                                    .unwrap();
                            }
                            if mode == 2 {
                                cancel.cancel();
                            }
                        });
                        true
                    },
                    |closed| {
                        let _ = tx.send(closed);
                    },
                    if mode == 0 {
                        std::time::Duration::ZERO
                    } else {
                        std::time::Duration::from_secs(2)
                    },
                )
                .await;
            assert_eq!(rx.await.unwrap().reason, expected);
            assert_eq!(result.is_ok(), mode == 1);
        }
    }

    #[test]
    fn approval_event_masks_known_and_pattern_secrets() {
        let secret = "fixture-opaque-secret-value";
        let mut request = browser_scope_request(
            "chat",
            "claude",
            "https://example.test",
            "skill",
            "publish",
            secret,
        );
        request.targets[0].path = format!("https://example.test/{secret}");
        request.reason = Some(secret.to_owned());
        request.redact_sensitive(&[secret]);
        let output = serde_json::to_string(&request).unwrap();
        assert!(!output.contains(secret));
        assert!(output.contains("example.test"));
    }

    #[test]
    fn dictionary_approval_is_bound_to_user_file_and_one_time_choices() {
        let request = dictionary_request("chat", "claude", "가상 표기 추가".into());
        assert_eq!(request.kind, "dictionary_change");
        assert_eq!(request.targets[0].path, "dictionary/my-terms.json");
        assert_eq!(
            request.options,
            vec![
                ApprovalChoice::AllowOnce,
                ApprovalChoice::Deny,
                ApprovalChoice::Cancel
            ]
        );
        assert!(request.description.contains("가상 표기"));
    }

    #[test]
    fn dictionary_delete_approval_is_one_time_and_targets_user_file() {
        let request = dictionary_delete_request("chat", "claude", "가상 용어 삭제".into());
        assert_eq!(request.kind, "dictionary_change");
        assert_eq!(request.change, Some(ChangeKind::Delete));
        assert_eq!(request.targets[0].path, "dictionary/my-terms.json");
        assert_eq!(request.targets[0].change, Some(ChangeKind::Delete));
        assert_eq!(
            request.options,
            vec![
                ApprovalChoice::AllowOnce,
                ApprovalChoice::Deny,
                ApprovalChoice::Cancel
            ]
        );
    }

    #[tokio::test]
    async fn setup_card_waits_for_one_user_response_and_cancel_closes_it() {
        for choice in [ApprovalChoice::AllowOnce, ApprovalChoice::Deny] {
            let broker = ApprovalBroker::default();
            let worker = broker.clone();
            let request = setup_request(
                "chat",
                "claude",
                SetupApproval {
                    action: "create_and_connect",
                    service: "github".into(),
                    account: "owner".into(),
                    region: None,
                    resource_name: "momso-teacher".into(),
                    visibility: "private",
                    cost: "possible",
                },
            );
            let id = request.request_id.clone();
            let (tx, rx) = oneshot::channel();
            let task = tokio::spawn(async move {
                worker
                    .request_with(
                        request,
                        CancellationToken::new(),
                        |card| {
                            let _ = tx.send(serde_json::to_value(card).unwrap());
                            true
                        },
                        |_| {},
                    )
                    .await
            });
            let card = rx.await.unwrap();
            assert_eq!(card["setup"]["resourceName"], "momso-teacher");
            assert!(!task.is_finished());
            broker.respond(&id, choice).await.unwrap();
            assert_eq!(task.await.unwrap().unwrap(), choice);
            assert!(broker.respond(&id, choice).await.is_err());
        }
        let broker = ApprovalBroker::default();
        let worker = broker.clone();
        let request = setup_request(
            "chat",
            "codex",
            SetupApproval {
                action: "create_and_connect",
                service: "github".into(),
                account: "owner".into(),
                region: None,
                resource_name: "momso-teacher".into(),
                visibility: "private",
                cost: "possible",
            },
        );
        let id = request.request_id.clone();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (shown_tx, shown_rx) = oneshot::channel();
        let (close_tx, close_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            worker
                .request_with(
                    request,
                    worker_cancel,
                    |_| {
                        let _ = shown_tx.send(());
                        true
                    },
                    |closed| {
                        let _ = close_tx.send(closed);
                    },
                )
                .await
        });
        shown_rx.await.unwrap();
        cancel.cancel();
        assert!(task.await.unwrap().is_err());
        assert_eq!(close_rx.await.unwrap().request_id, id);
        assert!(broker
            .respond(&id, ApprovalChoice::AllowOnce)
            .await
            .is_err());
    }

    #[test]
    fn setup_approval_binds_exact_remote_target_and_cost_without_secrets() {
        let request = setup_request(
            "chat-1",
            "claude",
            SetupApproval {
                action: "create_and_connect",
                service: "github".into(),
                account: "owner".into(),
                region: None,
                resource_name: "momso-test".into(),
                visibility: "private",
                cost: "possible",
            },
        );
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["chatId"], "chat-1");
        assert_eq!(value["kind"], "setup_change");
        assert_eq!(value["setup"]["account"], "owner");
        assert_eq!(value["setup"]["resourceName"], "momso-test");
        assert_eq!(value["setup"]["cost"], "possible");
        assert_eq!(
            value["options"],
            serde_json::json!(["allow_once", "deny", "cancel"])
        );
        assert!(value["setup"].get("secret").is_none());
    }

    #[test]
    fn request_and_close_include_chat_id_or_null() {
        let mut request = ApprovalRequest {
            request_id: "req".into(),
            chat_id: Some("chat_123".into()),
            provider: "codex".into(),
            kind: "command".into(),
            description: "cmd".into(),
            command: None,
            sandbox_escalation: None,
            change: None,
            targets: vec![],
            reason: None,
            options: vec![ApprovalChoice::Deny],
            setup: None,
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
                test_request("a", vec![ApprovalChoice::AllowOnce, ApprovalChoice::Deny]),
                CancellationToken::new(),
                std::time::Duration::from_secs(30),
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
            .register(
                test_request("a", vec![ApprovalChoice::Deny]),
                CancellationToken::new(),
                std::time::Duration::from_secs(30),
            )
            .await
            .unwrap();
        assert!(broker
            .register(
                test_request("a", vec![ApprovalChoice::Deny]),
                CancellationToken::new(),
                std::time::Duration::from_secs(30)
            )
            .await
            .is_err());
        assert!(broker
            .register(
                test_request("b", vec![]),
                CancellationToken::new(),
                std::time::Duration::from_secs(30)
            )
            .await
            .is_err());
    }

    fn test_request(id: &str, options: Vec<ApprovalChoice>) -> ApprovalRequest {
        let mut request = browser_request(
            "chat-a",
            "codex",
            "https://example.test",
            "click",
            "scope",
            "item",
        );
        request.request_id = id.into();
        request.options = options;
        request
    }

    #[tokio::test]
    async fn pending_only_returns_live_unanswered_requests_for_chat() {
        let broker = ApprovalBroker::default();
        let timeout = std::time::Duration::from_secs(30);
        let cancel = CancellationToken::new();
        let _a = broker
            .register(
                test_request("a", vec![ApprovalChoice::Deny]),
                CancellationToken::new(),
                timeout,
            )
            .await
            .unwrap();
        let mut second = test_request("b", vec![ApprovalChoice::Deny]);
        second.chat_id = Some("chat-b".into());
        let _b = broker
            .register(second, CancellationToken::new(), timeout)
            .await
            .unwrap();
        let _c = broker
            .register(
                test_request("c", vec![ApprovalChoice::Deny]),
                cancel.clone(),
                timeout,
            )
            .await
            .unwrap();
        let _expired = broker
            .register(
                test_request("expired", vec![ApprovalChoice::Deny]),
                CancellationToken::new(),
                std::time::Duration::ZERO,
            )
            .await
            .unwrap();
        cancel.cancel();
        assert_eq!(
            broker
                .pending(Some("chat-a"))
                .await
                .iter()
                .map(|r| r.request_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert_eq!(broker.pending(None).await.len(), 2);
        broker.respond("a", ApprovalChoice::Deny).await.unwrap();
        assert!(broker.pending(Some("chat-a")).await.is_empty());
        assert!(broker
            .respond("expired", ApprovalChoice::Deny)
            .await
            .is_err());
    }
}
