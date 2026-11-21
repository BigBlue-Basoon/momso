//! Chat-local panel identity; sites share a browser profile, not a page.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct State {
    active: Option<String>,
    labels: BTreeMap<String, String>,
    adopted: BTreeMap<String, String>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn label(chat_id: &str) -> Result<String, &'static str> {
    if chat_id.is_empty() || chat_id.len() > 128 || chat_id.chars().any(char::is_control) {
        return Err("invalid_chat");
    }
    Ok(format!(
        "panel-chat-{:x}",
        Sha256::digest(chat_id.as_bytes())
    ))
}

impl State {
    fn select(&mut self, chat_id: &str) -> Result<(Option<String>, String), &'static str> {
        if self.adopted.contains_key(chat_id) {
            return Err("draft_adopted");
        }
        let proposed = label(chat_id)?;
        let label = self
            .labels
            .entry(chat_id.to_owned())
            .or_insert(proposed)
            .clone();
        let previous = self.active.replace(chat_id.to_owned());
        Ok((previous, label))
    }

    fn adopt(&mut self, draft: &str, chat: &str) -> Result<(), &'static str> {
        if !draft
            .strip_prefix("draft-")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            || draft == chat
        {
            return Err("invalid_draft");
        }
        label(chat)?;
        if self.active.as_deref() != Some(draft) {
            return Err("panel_not_active");
        }
        if self.labels.contains_key(chat) {
            return Err("panel_exists");
        }
        let panel = self.labels.remove(draft).ok_or("invalid_draft")?;
        self.labels.insert(chat.to_owned(), panel);
        self.adopted.insert(draft.to_owned(), chat.to_owned());
        self.active = Some(chat.to_owned());
        Ok(())
    }
}

pub(crate) fn adopt(draft: &str, chat: &str) -> Result<(), &'static str> {
    state()
        .lock()
        .map_err(|_| "unavailable")?
        .adopt(draft, chat)
}

pub(crate) fn resolve(chat: Option<String>) -> Option<String> {
    let chat = chat?;
    let state = state().lock().ok()?;
    Some(state.adopted.get(&chat).cloned().unwrap_or(chat))
}

pub(crate) fn validate(chat_id: &str) -> Result<(), &'static str> {
    label(chat_id).map(|_| ())
}

pub(crate) fn select(chat_id: &str) -> Result<(Option<String>, String), &'static str> {
    let mut state = state().lock().map_err(|_| "unavailable")?;
    state.select(chat_id)
}

pub(crate) fn active_chat() -> Option<String> {
    state().lock().ok()?.active.clone()
}

pub(crate) fn active_label() -> Option<String> {
    let state = state().lock().ok()?;
    state.labels.get(state.active.as_ref()?).cloned()
}

pub(crate) fn labels() -> Vec<String> {
    state()
        .lock()
        .map(|state| state.labels.values().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_chat_has_a_distinct_stable_non_identifying_label() {
        let first = label("chat_first").unwrap();
        let second = label("chat_second").unwrap();
        assert_ne!(first, second);
        assert_eq!(first, label("chat_first").unwrap());
        assert!(!first.contains("chat_first"));
        assert!(label("").is_err());
        assert!(label("bad\nchat").is_err());
    }

    #[test]
    fn switching_back_recovers_only_that_chats_panel_identity() {
        let mut state = State::default();
        let (_, first) = state.select("chat_first").unwrap();
        let (previous, second) = state.select("chat_second").unwrap();
        assert_eq!(previous.as_deref(), Some("chat_first"));
        assert_ne!(first, second);
        let (previous, restored) = state.select("chat_first").unwrap();
        assert_eq!(previous.as_deref(), Some("chat_second"));
        assert_eq!(restored, first);
        assert_eq!(state.active.as_deref(), Some("chat_first"));
    }

    #[test]
    fn adopted_draft_keeps_its_panel_and_cannot_replace_an_existing_panel() {
        let mut state = State::default();
        let draft = format!("draft-{}", uuid::Uuid::new_v4());
        let (_, panel) = state.select(&draft).unwrap();
        state.adopt(&draft, "chat-new").unwrap();
        assert_eq!(state.select(&draft), Err("draft_adopted"));
        assert_eq!(state.select("chat-new").unwrap().1, panel);
        assert_eq!(
            state.adopted.get(&draft).map(String::as_str),
            Some("chat-new")
        );
        let next = format!("draft-{}", uuid::Uuid::new_v4());
        state.select(&next).unwrap();
        assert_eq!(state.adopt(&next, "chat-new"), Err("panel_exists"));
    }
}
