//! Host-only GitHub device authentication and local archive transport.
//! This library does not implement the server HTTP or authorization layer.
#[cfg(target_os = "linux")]
pub mod github;

#[cfg(target_os = "linux")]
#[path = "../../src/chat_archive_redact.rs"]
mod chat_archive_redact;

/// Make a safe copy for a chat backup without changing the local original.
#[cfg(target_os = "linux")]
pub fn redact_chat_copy(text: &str, known: &[&str]) -> (String, usize) {
    let value = chat_archive_redact::redact_with_known(text, known);
    (value.text, value.count)
}
