//! App-owned setup actions. Inputs and responses never contain credentials.
use serde_json::{json, Value};
use tauri::{Manager, Emitter};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tokio_util::sync::CancellationToken;

fn method(input: &Value) -> Option<&str> {
    let object = input.as_object()?;
    if object.len() != 1 {
        return None;
    }
    match object.get("method")?.as_str()? {
        "github_connect" => Some("github_connect"),
        _ => None,
    }
}

fn retry_reason(status: &str, cancelled: bool) -> Option<&'static str> {
    (status == "credentials_rejected_before_create" && !cancelled).then_some("invalid_credentials")
}

fn storage_tool_result(outcome: &Value) -> Value {
    match outcome.get("status").and_then(Value::as_str) {
        Some("connected") => json!({"status":"success"}),
        Some("cancelled") => json!({"status":"cancelled"}),
        _ => json!({"status":"failure"}),
    }
}

fn secret_request(
    chat_id: &str,
    service: &str,
    account: &str,
    resource_name: &str,
    reason: Option<&'static str>,
) -> crate::setup_secret::SecretRequest {
    crate::setup_secret::SecretRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat_id.into(),
        service: service.into(),
        account: account.into(),
        resource_name: resource_name.into(),
        fields: if service == "gcp" {
            vec!["accessKeyId", "secretAccessKey", "gcpOauthToken"]
        } else {
            vec!["accessKeyId", "secretAccessKey"]
        },
        reason,
    }
}

pub(crate) async fn dispatch(
    app: tauri::AppHandle,
    chat_id: String,
    ai_provider: String,
    cancel: CancellationToken,
    input: Value,
) -> Value {
    if input.as_object().is_some_and(|o| o.len() == 2) && input.get("method").and_then(Value::as_str) == Some("import_conversations") {
        let provider = match input.get("provider").and_then(Value::as_str) {
            Some("chatgpt") => "chatgpt", Some("claude") => "claude",
            Some("gemini") => return json!({"status":"unsupported_format"}),
            _ => return json!({"status":"unavailable"}),
        };
        let db = app.state::<crate::AppState>().paths.database_path.clone();
        let lease = app.state::<crate::AppState>().paths.archive.current.clone().read_owned().await;
        return tauri::async_runtime::spawn_blocking(move || {
            let _lease = lease;
            if cancel.is_cancelled() { return json!({"status":"cancelled"}); }
            let Some(file) = app.dialog().file().add_filter("대화 내보내기", &["json", "zip"]).blocking_pick_file().and_then(|v| v.into_path().ok()) else { return json!({"status":"cancelled"}); };
            if cancel.is_cancelled() { return json!({"status":"cancelled"}); }
            let preview = match crate::chat_export_import::preview(&db, &file, provider) { Ok(v) => v, Err(_) => return json!({"status":"failure"}) };
            let message = format!("대화 {}개, 이미 가져온 대화 {}개, 읽을 수 없는 대화 {}개입니다. 가져올까요?", preview.conversations, preview.already_imported, preview.unreadable);
            if !app.dialog().message(message).buttons(MessageDialogButtons::OkCancel).blocking_show() || cancel.is_cancelled() { return json!({"status":"cancelled"}); }
            let request_id = uuid::Uuid::new_v4().to_string();
            let result = crate::chat_export_import::import_with_id(&db, &file, provider, &request_id, &|processed| {
                if cancel.is_cancelled() { crate::chat_export_import::cancel(&request_id); }
                let _ = app.emit("chat-import-progress", json!({"requestId":request_id,"processed":processed,"chatId":chat_id}));
            });
            match result { Ok(v) => json!({"status":"success","imported":v.imported,"duplicate":v.duplicate,"skipped":v.skipped}), Err(e) if e == "cancelled" => json!({"status":"cancelled"}), Err(_) => json!({"status":"failure"}) }
        }).await.unwrap_or_else(|_| json!({"status":"unavailable"}));
    }
    if let Some((service, account, region)) = storage_account(&input) {
        let outcome = connect_storage(
            app,
            chat_id,
            ai_provider,
            cancel,
            service,
            account.to_owned(),
            region.to_owned(),
        )
        .await;
        return storage_tool_result(&outcome);
    }
    if method(&input) != Some("github_connect") || cancel.is_cancelled() {
        return json!({"status":"unavailable"});
    }
    let db = app.state::<crate::AppState>().paths.database_path.clone();
    let candidate =
        tauri::async_runtime::spawn_blocking(move || crate::archive_remote::setup_candidate(&db))
            .await;
    let (account, resource) = match candidate {
        Ok(Ok(value)) => value,
        Ok(Err(status)) => return json!({"status":status}),
        Err(_) => return json!({"status":"unavailable"}),
    };
    let Some(username) = resource.strip_prefix("momso-") else {
        return json!({"status":"unavailable"});
    };
    let plan = match crate::setup_approval::SetupPlan::new("github", username, &account, None) {
        Ok(value) => value,
        Err(status) => return json!({"status":status}),
    };
    let broker = app.state::<crate::AppState>().approvals.clone();
    let permit = match plan
        .authorize(&broker, &app, &chat_id, &ai_provider, cancel.clone())
        .await
    {
        Ok(value) => value,
        Err(status) => return json!({"status":status}),
    };
    let result =
        crate::archive_remote::create_approved(app.clone(), app.state(), &chat_id, permit, cancel)
            .await;
    match result
        .ok()
        .and_then(|v| serde_json::to_value(v).ok())
        .and_then(|v| v.get("state").and_then(Value::as_str).map(str::to_owned))
    {
        Some(status) => json!({"status":status}),
        None => json!({"status":"unavailable"}),
    }
}

pub(crate) fn storage_account(input: &Value) -> Option<(&'static str, &str, &str)> {
    let object = input.as_object()?;
    let account = object.get("account")?.as_str()?;
    match object.get("method")?.as_str()? {
        "object_store_r2_connect"
            if object.len() == 2
                && account.len() == 32
                && account.bytes().all(|b| b.is_ascii_hexdigit()) =>
        {
            Some(("cloudflare", account, "auto"))
        }
        "object_store_gcp_connect"
            if object.len() == 2
                && (6..=30).contains(&account.len())
                && account
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') =>
        {
            Some(("gcp", account, "auto"))
        }
        "object_store_aws_connect"
            if object.len() == 3
                && account.len() == 12
                && account.bytes().all(|b| b.is_ascii_digit()) =>
        {
            let region = object.get("region")?.as_str()?;
            if region.is_empty()
                || region == "us-east-1"
                || region.len() > 32
                || !region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return None;
            }
            Some(("aws", account, region))
        }
        _ => None,
    }
}

async fn connect_storage(
    app: tauri::AppHandle,
    chat_id: String,
    ai_provider: String,
    cancel: CancellationToken,
    service: &'static str,
    account: String,
    region: String,
) -> Value {
    if cancel.is_cancelled() {
        return json!({"status":"cancelled"});
    }
    let db = app.state::<crate::AppState>().paths.database_path.clone();
    let username = match crate::archive_remote::username_get_db(&db) {
        Ok(Some(value)) => value,
        _ => return json!({"status":"username_missing"}),
    };
    let resource_name = match crate::object_store::bucket::automatic_name(service, Some(&username))
    {
        Ok(value) => value,
        Err(status) => return json!({"status":status}),
    };
    let secrets = app.state::<crate::AppState>().setup_secrets.clone();
    let broker = app.state::<crate::AppState>().approvals.clone();
    let secret_app = app.clone();
    let approval_app = app.clone();
    let connection_app = app.clone();
    let secret_cancel = cancel.clone();
    let approval_cancel = cancel.clone();
    let connection_cancel = cancel.clone();
    let approval_chat = chat_id.clone();
    let connection_chat = chat_id.clone();
    run_storage_flow(
        &chat_id,
        &username,
        service,
        &account,
        &region,
        &resource_name,
        cancel,
        move |request| {
            let broker = secrets.clone();
            let app = secret_app.clone();
            let cancel = secret_cancel.clone();
            async move { broker.request(&app, request, cancel).await }
        },
        move |plan| {
            let broker = broker.clone();
            let app = approval_app.clone();
            let chat = approval_chat.clone();
            let provider = ai_provider.clone();
            let cancel = approval_cancel.clone();
            async move {
                plan.authorize(&broker, &app, &chat, &provider, cancel)
                    .await
            }
        },
        move |permit, secret| {
            let app = connection_app.clone();
            let chat = connection_chat.clone();
            let cancel = connection_cancel.clone();
            async move {
                crate::object_store::connect_approved(app.state(), &chat, permit, secret, cancel)
                    .await
            }
        },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_storage_flow<S, P, FS, FA, FC, SF, AF, CF>(
    chat_id: &str,
    username: &str,
    service: &str,
    account: &str,
    region: &str,
    resource_name: &str,
    cancel: CancellationToken,
    mut secret_input: FS,
    mut approve: FA,
    mut connect: FC,
) -> Value
where
    FS: FnMut(crate::setup_secret::SecretRequest) -> SF,
    SF: std::future::Future<Output = Result<S, &'static str>>,
    FA: FnMut(crate::setup_approval::SetupPlan) -> AF,
    AF: std::future::Future<Output = Result<P, &'static str>>,
    FC: FnMut(P, S) -> CF,
    CF: std::future::Future<Output = Result<(), &'static str>>,
{
    let mut reason = None;
    loop {
        let plan =
            match crate::setup_approval::SetupPlan::new(service, username, account, Some(region)) {
                Ok(value) => value,
                Err(status) => return json!({"status":status}),
            };
        let request = secret_request(chat_id, service, account, resource_name, reason);
        let secret = match secret_input(request).await {
            Ok(value) => value,
            Err(status) => return json!({"status":status}),
        };
        let permit = match approve(plan).await {
            Ok(value) => value,
            Err(status) => return json!({"status":status}),
        };
        match connect(permit, secret).await {
            Ok(()) => return json!({"status":"connected"}),
            Err(status) => match retry_reason(status, cancel.is_cancelled()) {
                Some(value) => reason = Some(value),
                None => return json!({"status":status}),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn storage_result_reports_only_cancellation_or_success_or_failure() {
        assert_eq!(
            storage_tool_result(&json!({"status":"cancelled", "secret":"do-not-return"})),
            json!({"status":"cancelled"})
        );
        assert_eq!(
            storage_tool_result(&json!({"status":"connected"})),
            json!({"status":"success"})
        );
        assert_eq!(
            storage_tool_result(&json!({"status":"unavailable"})),
            json!({"status":"failure"})
        );
    }
    #[test]
    fn retries_only_explicit_pre_creation_credential_errors() {
        assert_eq!(
            retry_reason("credentials_rejected_before_create", false),
            Some("invalid_credentials")
        );
        for status in [
            "account_unverified",
            "created_not_connected",
            "access_denied",
            "unavailable",
        ] {
            assert_eq!(retry_reason(status, false), None);
        }
        assert_eq!(
            retry_reason("credentials_rejected_before_create", true),
            None
        );
        let first = secret_request("room", "gcp", "project", "momso-teacher", None);
        let retry = secret_request(
            "room",
            "gcp",
            "project",
            "momso-teacher",
            Some("invalid_credentials"),
        );
        assert_ne!(first.request_id, retry.request_id);
        let event = serde_json::to_value(retry).unwrap();
        assert_eq!(event["chatId"], "room");
        assert_eq!(event["reason"], "invalid_credentials");
        assert_eq!(
            event["fields"],
            json!(["accessKeyId", "secretAccessKey", "gcpOauthToken"])
        );
    }
    #[test]
    fn rejects_any_arguments_or_credentials() {
        assert_eq!(
            method(&json!({"method":"github_connect"})),
            Some("github_connect")
        );
        assert_eq!(
            method(&json!({"method":"github_connect","secret":"test-key"})),
            None
        );
        assert_eq!(method(&json!({"method":"shell"})), None);
        let account = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            storage_account(&json!({"method":"object_store_r2_connect","account":account})),
            Some(("cloudflare", account, "auto"))
        );
        assert!(storage_account(
            &json!({"method":"object_store_r2_connect","account":account,"secret":"test-key"})
        )
        .is_none());
        assert_eq!(
            storage_account(&json!({"method":"object_store_gcp_connect","account":"test-project"})),
            Some(("gcp", "test-project", "auto"))
        );
        assert_eq!(
            storage_account(
                &json!({"method":"object_store_aws_connect","account":"123456789012","region":"ap-northeast-2"})
            ),
            Some(("aws", "123456789012", "ap-northeast-2"))
        );
    }
}
