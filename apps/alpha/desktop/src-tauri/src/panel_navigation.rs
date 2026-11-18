//! Origin checks used at the host side of a controlled panel session.
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    generation: u64,
    origins: Option<BTreeSet<String>>,
    blocked: Option<String>,
}

/// Inactive means ordinary human navigation. Active means exact origin checks.
#[derive(Clone, Default)]
pub struct PanelNavigation(Arc<Mutex<State>>);

/// Clears only its own generation; an obsolete lease cannot release a newer policy.
pub struct NavigationLease {
    gate: PanelNavigation,
    generation: u64,
}

fn origin(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn validated_origin(input: &str) -> Result<String, &'static str> {
    let url = reqwest::Url::parse(input).map_err(|_| "invalid_origin")?;
    let test_loopback = cfg!(test) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    if (url.scheme() != "https" && !test_loopback)
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("invalid_origin");
    }
    origin(input).ok_or("invalid_origin")
}

impl PanelNavigation {
    pub fn activate(&self, origins: &[String]) -> Result<NavigationLease, &'static str> {
        let mut allowed = BTreeSet::new();
        for input in origins {
            allowed.insert(validated_origin(input)?);
        }
        if allowed.is_empty() {
            return Err("invalid_origin");
        }
        let mut state = self.0.lock().map_err(|_| "unavailable")?;
        if state.origins.is_some() {
            return Err("panel_busy");
        }
        state.generation = state.generation.checked_add(1).ok_or("unavailable")?;
        state.origins = Some(allowed);
        state.blocked = None;
        Ok(NavigationLease {
            gate: self.clone(),
            generation: state.generation,
        })
    }

    /// When inactive, the human panel's own existing URL checks remain authoritative.
    pub fn permits_navigation(&self, target: &str) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        let candidate = origin(target);
        let allowed = state.origins.as_ref().is_none_or(|allowed| {
            candidate
                .as_ref()
                .is_some_and(|value| allowed.contains(value))
        });
        if !allowed {
            state.blocked = Some(if crate::browser_runtime::google_login_target(target) {
                target.to_owned()
            } else {
                candidate.unwrap_or_default()
            });
        }
        allowed
    }

    /// Check subresource requests without reporting them as a main-frame navigation.
    pub fn permits_request(&self, target: &str) -> bool {
        let Ok(state) = self.0.lock() else {
            return false;
        };
        state.origins.as_ref().is_none_or(|allowed| {
            origin(target).is_some_and(|candidate| allowed.contains(&candidate))
        })
    }

    /// AI-controlled sessions deny new windows; the human panel keeps its old behavior.
    pub fn permits_popup(&self) -> bool {
        self.0.lock().is_ok_and(|mut state| {
            if state.origins.is_some() {
                state.blocked = Some("new_window".into());
                false
            } else {
                true
            }
        })
    }

    pub fn blocked(&self) -> Option<String> {
        self.0.lock().ok().and_then(|state| state.blocked.clone())
    }
    pub fn mark_external_login(&self, target: &str) {
        if let Ok(mut state) = self.0.lock() {
            if state.origins.is_some() && crate::browser_runtime::google_login_target(target) {
                state.blocked = Some(target.to_owned());
            }
        }
    }

    pub fn clear_blocked(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.blocked = None;
        }
    }
}

impl NavigationLease {
    /// Only the host's already-approved session may extend this lease.
    pub fn allow_origin(&self, input: &str) -> Result<(), &'static str> {
        let value = validated_origin(input)?;
        let mut state = self.gate.0.lock().map_err(|_| "unavailable")?;
        if state.generation != self.generation {
            return Err("panel_closed");
        }
        state.origins.as_mut().ok_or("panel_closed")?.insert(value);
        Ok(())
    }
}

impl Drop for NavigationLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.gate.0.lock() {
            if state.generation == self.generation {
                state.origins = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_google_login_retains_exact_address_only_for_google() {
        let gate = PanelNavigation::default();
        let _lease = gate.activate(&["https://site.example".into()]).unwrap();
        let login = "https://accounts.google.com/signin?client_id=fixture&state=fixture";
        assert!(!gate.permits_navigation(login));
        assert_eq!(gate.blocked().as_deref(), Some(login));
        gate.mark_external_login(login);
        assert_eq!(gate.blocked().as_deref(), Some(login));
        assert!(!gate.permits_navigation("https://outside.example/private?token=fixture"));
        assert_eq!(gate.blocked().as_deref(), Some("https://outside.example"));
    }

    #[test]
    fn active_gate_requires_exact_origin_and_denies_popups() {
        let gate = PanelNavigation::default();
        assert!(gate.permits_popup());
        let lease = gate.activate(&["https://site.example".into()]).unwrap();
        assert!(gate.permits_navigation("https://site.example/lesson"));
        for url in [
            "https://site.example.attacker.test/",
            "https://site.example:444/",
            "https://site.example@attacker.test/",
            "file:///tmp/a",
            "javascript:alert(1)",
        ] {
            assert!(!gate.permits_navigation(url), "{url}");
        }
        assert!(!gate.permits_popup());
        drop(lease);
        assert!(gate.permits_popup());
    }

    #[test]
    fn policy_must_be_exact_origins_and_cannot_be_replaced_while_active() {
        let gate = PanelNavigation::default();
        for value in [
            concat!("https://user:pass", "@site.example"),
            "https://site.example/path",
            "http://site.example",
            "https://site.example/?query=1",
        ] {
            assert!(matches!(
                gate.activate(&[value.into()]),
                Err("invalid_origin")
            ));
        }
        let lease = gate.activate(&["https://site.example".into()]).unwrap();
        assert!(matches!(
            gate.activate(&["https://other.example".into()]),
            Err("panel_busy")
        ));
        drop(lease);
        assert!(gate.activate(&["https://other.example".into()]).is_ok());
    }

    #[test]
    fn only_current_lease_extends_a_normalized_origin() {
        let gate = PanelNavigation::default();
        let lease = gate.activate(&["https://site.example".into()]).unwrap();
        assert!(!gate.permits_navigation("https://login.example/path"));
        assert_eq!(
            lease.allow_origin("https://login.example/path"),
            Err("invalid_origin")
        );
        lease.allow_origin("https://login.example").unwrap();
        assert!(gate.permits_navigation("https://login.example/path"));
        assert!(!gate.permits_navigation("https://login.example.attacker.test"));
        let stale = NavigationLease {
            gate: gate.clone(),
            generation: lease.generation,
        };
        drop(lease);
        let next = gate.activate(&["https://site.example".into()]).unwrap();
        assert_eq!(
            stale.allow_origin("https://stale.example"),
            Err("panel_closed")
        );
        drop(stale);
        assert!(!gate.permits_navigation("https://stale.example/path"));
        assert!(!gate.permits_navigation("https://login.example/path"));
        drop(next);
    }
}
