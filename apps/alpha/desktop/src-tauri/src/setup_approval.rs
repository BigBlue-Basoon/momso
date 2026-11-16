//! Host-owned, one-use approval for a concrete setup operation.
#![allow(dead_code)] // Not wired to MCP until the child-to-host boundary is authenticated.
use crate::approval::{self, ApprovalBroker, ApprovalChoice, SetupApproval};
use tokio_util::sync::CancellationToken;

pub(crate) struct SetupPlan {
    details: SetupApproval,
}
impl SetupPlan {
    pub(crate) fn new(
        service: &str,
        username: &str,
        account: &str,
        region: Option<&str>,
    ) -> Result<Self, &'static str> {
        if account.trim().is_empty()
            || account.trim() != account
            || account.len() > 256
            || account.chars().any(char::is_control)
        {
            return Err("invalid_account");
        }
        let resource_name = if service == "github" {
            if !crate::archive_remote::valid_username(username) {
                return Err("invalid_username");
            }
            format!("momso-{username}")
        } else {
            crate::object_store::bucket::automatic_name(service, Some(username))?
        };
        if service != "github"
            && region.is_none_or(|s| {
                s.is_empty()
                    || s.len() > 128
                    || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err("invalid_region");
        }
        Ok(Self {
            details: SetupApproval {
                action: "create_and_connect",
                service: service.into(),
                account: account.into(),
                region: region.map(str::to_owned),
                resource_name,
                visibility: "private",
                cost: "possible",
            },
        })
    }
    pub(crate) async fn authorize(
        self,
        broker: &ApprovalBroker,
        app: &tauri::AppHandle,
        chat_id: &str,
        ai_provider: &str,
        cancellation: CancellationToken,
    ) -> Result<SetupPermit, &'static str> {
        let request = approval::setup_request(chat_id, ai_provider, self.details.clone());
        let choice = broker
            .request(app, request, cancellation.clone())
            .await
            .map_err(|_| "unavailable")?;
        self.decide(choice, chat_id, cancellation)
    }
    #[cfg(test)]
    pub(crate) async fn authorize_with(
        self,
        broker: &ApprovalBroker,
        chat_id: &str,
        ai_provider: &str,
        cancellation: CancellationToken,
        emit: impl FnOnce(&approval::ApprovalRequest) -> bool,
        close: impl FnOnce(approval::ApprovalClosed),
    ) -> Result<SetupPermit, &'static str> {
        let request = approval::setup_request(chat_id, ai_provider, self.details.clone());
        let choice = broker
            .request_with(request, cancellation.clone(), emit, close)
            .await
            .map_err(|_| "unavailable")?;
        self.decide(choice, chat_id, cancellation)
    }
    fn decide(
        self,
        choice: ApprovalChoice,
        chat_id: &str,
        cancellation: CancellationToken,
    ) -> Result<SetupPermit, &'static str> {
        match choice {
            ApprovalChoice::AllowOnce if !chat_id.is_empty() && !cancellation.is_cancelled() => {
                Ok(SetupPermit {
                    plan: self,
                    chat_id: chat_id.into(),
                    expires: std::time::Instant::now() + std::time::Duration::from_secs(60),
                    cancellation,
                })
            }
            ApprovalChoice::AllowOnce => Err("unavailable"),
            ApprovalChoice::Deny => Err("denied"),
            ApprovalChoice::Cancel => Err("cancelled"),
            ApprovalChoice::AllowSession => Err("unavailable"),
        }
    }
}

// No Clone: one approval can only be consumed by one executor.
pub(crate) struct SetupPermit {
    plan: SetupPlan,
    chat_id: String,
    expires: std::time::Instant,
    cancellation: CancellationToken,
}
impl SetupPermit {
    pub(crate) fn consume(self, chat_id: &str) -> Result<SetupApproval, &'static str> {
        if self.chat_id != chat_id
            || std::time::Instant::now() >= self.expires
            || self.cancellation.is_cancelled()
        {
            return Err("unavailable");
        }
        Ok(self.plan.details)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plans_cannot_choose_arbitrary_resource_names() {
        let plan = SetupPlan::new("github", "teacher", "owner", None).unwrap();
        assert_eq!(plan.details.resource_name, "momso-teacher");
        assert!(SetupPlan::new("github", "../private", "owner", None).is_err());
        let bucket = SetupPlan::new("cloudflare", "teacher", "account", Some("auto")).unwrap();
        assert_eq!(bucket.details.resource_name, "momso-teacher");
        assert!(SetupPlan::new("cloudflare", "teacher", "", Some("auto")).is_err());
        assert!(SetupPlan::new("cloudflare", "teacher", "account\nother", Some("auto")).is_err());
        assert!(SetupPlan::new("aws", "teacher", "account", Some("region\nother")).is_err());
        assert!(SetupPlan::new("s3_compatible", "teacher", "account", Some("auto")).is_err());
    }
    #[test]
    fn only_single_use_approval_creates_a_bound_permit() {
        for choice in [
            ApprovalChoice::Deny,
            ApprovalChoice::Cancel,
            ApprovalChoice::AllowSession,
        ] {
            assert!(SetupPlan::new("github", "teacher", "owner", None)
                .unwrap()
                .decide(choice, "chat-1", CancellationToken::new())
                .is_err());
        }
        let permit = SetupPlan::new("github", "teacher", "owner", None)
            .unwrap()
            .decide(
                ApprovalChoice::AllowOnce,
                "chat-1",
                CancellationToken::new(),
            )
            .unwrap();
        let details = permit.consume("chat-1").unwrap();
        assert_eq!(details.account, "owner");
        assert_eq!(details.resource_name, "momso-teacher");
        let permit = SetupPlan::new("github", "teacher", "owner", None)
            .unwrap()
            .decide(
                ApprovalChoice::AllowOnce,
                "chat-1",
                CancellationToken::new(),
            )
            .unwrap();
        assert!(permit.consume("other-chat").is_err());
        let cancel = CancellationToken::new();
        let permit = SetupPlan::new("github", "teacher", "owner", None)
            .unwrap()
            .decide(ApprovalChoice::AllowOnce, "chat-1", cancel.clone())
            .unwrap();
        cancel.cancel();
        assert!(permit.consume("chat-1").is_err());
    }
}
