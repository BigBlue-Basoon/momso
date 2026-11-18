//! Host-side ownership of the single panel; no web page or AI chooses its owner.
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct State {
    owner: Option<String>,
    generation: u64,
    taken_over: HashSet<String>,
    closed: HashSet<String>,
    cancellation: Option<CancellationToken>,
}

/// Shared by panel commands and sessions. A lease never authorizes a page action itself.
#[derive(Clone, Default)]
pub struct PanelOwnership(Arc<Mutex<State>>);

/// A generation-bound lease cannot become valid again after a human takeover.
pub struct PanelLease {
    ownership: PanelOwnership,
    chat_id: String,
    generation: u64,
    cancellation: CancellationToken,
}

impl PanelOwnership {
    /// Claim the panel after the user has approved the skill's site scope.
    pub fn claim(&self, chat_id: &str) -> Result<PanelLease, &'static str> {
        if chat_id.is_empty() {
            return Err("unavailable");
        }
        let mut state = self.0.lock().map_err(|_| "unavailable")?;
        if state.taken_over.contains(chat_id) {
            return Err("human_takeover");
        }
        if state.owner.is_some() {
            return Err("panel_busy");
        }
        state.generation = state.generation.checked_add(1).ok_or("unavailable")?;
        state.owner = Some(chat_id.to_owned());
        state.closed.remove(chat_id);
        let cancellation = CancellationToken::new();
        state.cancellation = Some(cancellation.clone());
        Ok(PanelLease {
            ownership: self.clone(),
            chat_id: chat_id.to_owned(),
            generation: state.generation,
            cancellation,
        })
    }

    /// Returns the chat whose current lease was revoked, if any.
    pub fn take_over(&self) -> Result<Option<String>, &'static str> {
        let mut state = self.0.lock().map_err(|_| "unavailable")?;
        let owner = state.owner.take();
        if let Some(owner) = &owner {
            state.taken_over.insert(owner.clone());
        }
        let cancellation = state.cancellation.take();
        drop(state);
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
        }
        Ok(owner)
    }

    /// Prevent this chat from claiming the panel until the user delegates again.
    pub fn take_over_chat(&self, chat_id: &str) -> Result<(), &'static str> {
        if chat_id.is_empty() {
            return Err("unavailable");
        }
        let mut state = self.0.lock().map_err(|_| "unavailable")?;
        if state.owner.as_deref().is_some_and(|owner| owner != chat_id) {
            return Err("panel_busy");
        }
        state.taken_over.insert(chat_id.to_owned());
        state.owner = None;
        let cancellation = state.cancellation.take();
        drop(state);
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
        }
        Ok(())
    }

    pub fn human_controls(&self, chat_id: &str) -> Result<bool, &'static str> {
        Ok(self
            .0
            .lock()
            .map_err(|_| "unavailable")?
            .taken_over
            .contains(chat_id))
    }

    pub fn panel_closed(&self) -> Result<Option<String>, &'static str> {
        let mut state = self.0.lock().map_err(|_| "unavailable")?;
        let owner = state.owner.take();
        if let Some(owner) = &owner {
            state.closed.insert(owner.clone());
        }
        let cancellation = state.cancellation.take();
        drop(state);
        if let Some(cancellation) = cancellation {
            cancellation.cancel();
        }
        Ok(owner)
    }

    pub fn was_closed(&self, chat_id: &str) -> Result<bool, &'static str> {
        Ok(self
            .0
            .lock()
            .map_err(|_| "unavailable")?
            .closed
            .contains(chat_id))
    }

    /// Only an explicit human action should call this method.
    pub fn resume(&self, chat_id: &str) -> Result<(), &'static str> {
        if chat_id.is_empty() {
            return Err("unavailable");
        }
        self.0
            .lock()
            .map_err(|_| "unavailable")?
            .taken_over
            .remove(chat_id);
        Ok(())
    }

    pub fn owner(&self) -> Result<Option<String>, &'static str> {
        Ok(self.0.lock().map_err(|_| "unavailable")?.owner.clone())
    }
}

impl PanelLease {
    /// Pending approvals may use this token to close when control is taken back.
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Recheck immediately after every approval, before touching the page.
    pub fn check(&self) -> Result<(), &'static str> {
        let state = self.ownership.0.lock().map_err(|_| "unavailable")?;
        if state.taken_over.contains(&self.chat_id) {
            return Err("human_takeover");
        }
        if state.owner.as_deref() != Some(&self.chat_id) || state.generation != self.generation {
            return Err("panel_closed");
        }
        Ok(())
    }
}

impl Drop for PanelLease {
    fn drop(&mut self) {
        let should_cancel = if let Ok(mut state) = self.ownership.0.lock() {
            if state.owner.as_deref() == Some(&self.chat_id) && state.generation == self.generation
            {
                state.owner = None;
                state.cancellation = None;
                true
            } else {
                false
            }
        } else {
            false
        };
        if should_cancel {
            self.cancellation.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_chat_only_and_human_takeover_requires_explicit_resume() {
        let ownership = PanelOwnership::default();
        let first = ownership.claim("chat-a").unwrap();
        let pending = first.cancellation();
        assert!(matches!(ownership.claim("chat-b"), Err("panel_busy")));
        assert_eq!(ownership.take_over().unwrap().as_deref(), Some("chat-a"));
        assert!(pending.is_cancelled());
        assert_eq!(first.check(), Err("human_takeover"));
        assert!(matches!(ownership.claim("chat-a"), Err("human_takeover")));
        let second = ownership.claim("chat-b").unwrap();
        drop(first);
        assert_eq!(second.check(), Ok(()));
        ownership.resume("chat-a").unwrap();
        assert!(matches!(ownership.claim("chat-a"), Err("panel_busy")));
        drop(second);
        assert!(ownership.claim("chat-a").is_ok());
    }

    #[test]
    fn resumed_chat_never_revalidates_the_old_lease() {
        let ownership = PanelOwnership::default();
        let old = ownership.claim("chat-a").unwrap();
        ownership.take_over().unwrap();
        ownership.resume("chat-a").unwrap();
        let new = ownership.claim("chat-a").unwrap();
        let current = new.cancellation();
        assert_eq!(old.check(), Err("panel_closed"));
        drop(old);
        assert!(!current.is_cancelled());
        assert_eq!(new.check(), Ok(()));
    }

    #[test]
    fn idle_takeover_requires_explicit_resume_and_preserves_other_owner() {
        let ownership = PanelOwnership::default();
        ownership.take_over_chat("chat-a").unwrap();
        assert!(ownership.human_controls("chat-a").unwrap());
        assert!(matches!(ownership.claim("chat-a"), Err("human_takeover")));
        let lease = ownership.claim("chat-b").unwrap();
        assert_eq!(ownership.take_over_chat("chat-a"), Err("panel_busy"));
        assert_eq!(lease.check(), Ok(()));
        drop(lease);
        ownership.resume("chat-a").unwrap();
        assert!(!ownership.human_controls("chat-a").unwrap());
    }
}
