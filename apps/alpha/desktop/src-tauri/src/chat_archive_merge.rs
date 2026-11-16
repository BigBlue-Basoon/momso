//! Conservative, lossless message merge used by the Git chat backup work.
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Message {
    pub id: String,
    pub role: String,
    pub text: String,
    pub created_at: String,
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergeError {
    InvalidId,
    ConflictingMessage,
    OrderingConflict,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg(test)]
pub(crate) struct Deletion {
    pub chat_id: String,
    pub observed: BTreeMap<String, String>,
}

#[cfg(test)]
fn fingerprint(message: &Message) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(message).expect("message serializes"))
    )
}

/// Call only with the already-redacted copy, never local credential-bearing text.
#[cfg(test)]
pub(crate) fn mark_deleted(chat_id: &str, messages: &[Message]) -> Result<Deletion, MergeError> {
    if chat_id.is_empty() {
        return Err(MergeError::InvalidId);
    }
    let messages = merge_messages(messages, &[])?;
    Ok(Deletion {
        chat_id: chat_id.to_owned(),
        observed: messages
            .iter()
            .map(|m| (m.id.clone(), fingerprint(m)))
            .collect(),
    })
}

/// A new or changed message on another device must block automatic deletion.
#[cfg(test)]
pub(crate) fn deletion_matches(deletion: &Deletion, chat_id: &str, messages: &[Message]) -> bool {
    deletion.chat_id == chat_id
        && messages.iter().all(|message| {
            deletion
                .observed
                .get(&message.id)
                .is_some_and(|hash| hash == &fingerprint(message))
        })
}

/// Merge by immutable message identity. A repeated identity with different
/// contents is a conflict, never a reason to overwrite either side.
pub(crate) fn merge_messages(
    left: &[Message],
    right: &[Message],
) -> Result<Vec<Message>, MergeError> {
    let mut messages = BTreeMap::new();
    for message in left.iter().chain(right) {
        if message.id.is_empty() || message.id.len() > 128 {
            return Err(MergeError::InvalidId);
        }
        if let Some(previous) = messages.insert(message.id.clone(), message) {
            if previous != message {
                return Err(MergeError::ConflictingMessage);
            }
        }
    }
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut indegree: BTreeMap<String, usize> = messages.keys().map(|id| (id.clone(), 0)).collect();
    for side in [left, right] {
        for pair in side.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(MergeError::OrderingConflict);
            }
            if edges
                .entry(pair[0].id.clone())
                .or_default()
                .insert(pair[1].id.clone())
            {
                *indegree.get_mut(&pair[1].id).expect("message exists") += 1;
            }
        }
    }
    let order_key = |id: &String| {
        let time = chrono::DateTime::parse_from_rfc3339(&messages[id].created_at)
            .ok()
            .map(|time| time.with_timezone(&chrono::Utc));
        Reverse((time, id.clone()))
    };
    let mut ready: BinaryHeap<_> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| order_key(id))
        .collect();
    let mut result = Vec::with_capacity(messages.len());
    while let Some(Reverse((_, id))) = ready.pop() {
        result.push(messages[&id].clone());
        if let Some(next) = edges.get(&id) {
            for child in next {
                let degree = indegree.get_mut(child).expect("message exists");
                *degree -= 1;
                if *degree == 0 {
                    ready.push(order_key(child));
                }
            }
        }
    }
    if result.len() != messages.len() {
        return Err(MergeError::OrderingConflict);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn message(id: &str, text: &str, time: &str) -> Message {
        Message {
            id: id.into(),
            role: "user".into(),
            text: text.into(),
            created_at: time.into(),
            metadata: BTreeMap::new(),
        }
    }
    #[test]
    fn two_devices_keep_both_new_messages_and_do_not_duplicate_shared_messages() {
        let shared = message("shared", "original", "2026-01-01T00:00:00Z");
        let a = message("a", "one", "2026-01-02T00:00:00Z");
        let b = message("b", "two", "2026-01-02T00:00:00Z");
        let result =
            merge_messages(&[shared.clone(), a.clone()], &[shared.clone(), b.clone()]).unwrap();
        assert_eq!(result, vec![shared, a, b]);
    }
    #[test]
    fn changed_message_with_same_identity_is_never_silently_overwritten() {
        let left = message("shared", "left", "2026-01-01T00:00:00Z");
        let right = message("shared", "right", "2026-01-01T00:00:00Z");
        assert_eq!(
            merge_messages(&[left], &[right]),
            Err(MergeError::ConflictingMessage)
        );
    }
    #[test]
    fn preserves_each_device_order_even_with_clock_skew() {
        let a = message("a", "first", "2026-01-02T00:00:00Z");
        let b = message("b", "second", "2026-01-01T00:00:00Z");
        assert_eq!(
            merge_messages(&[a.clone(), b.clone()], &[]).unwrap(),
            vec![a, b]
        );
    }
    #[test]
    fn contradictory_device_order_is_rejected() {
        let a = message("a", "first", "2026-01-01T00:00:00Z");
        let b = message("b", "second", "2026-01-02T00:00:00Z");
        assert_eq!(
            merge_messages(&[a.clone(), b.clone()], &[b, a]),
            Err(MergeError::OrderingConflict)
        );
    }
    #[test]
    fn deletion_does_not_erase_another_devices_new_or_changed_messages() {
        let old = message("a", "first", "2026-01-01T00:00:00Z");
        let marker = mark_deleted("chat-test", std::slice::from_ref(&old)).unwrap();
        assert!(deletion_matches(
            &marker,
            "chat-test",
            std::slice::from_ref(&old)
        ));
        let new = message("b", "new", "2026-01-02T00:00:00Z");
        assert!(!deletion_matches(&marker, "chat-test", &[old.clone(), new]));
        let altered = message("a", "changed", "2026-01-01T00:00:00Z");
        assert!(!deletion_matches(&marker, "chat-test", &[altered]));
        assert!(!deletion_matches(&marker, "other-chat", &[old]));
    }
}
