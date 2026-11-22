//! Common insertion of a chat message inside the caller's transaction.
use rusqlite::{params, Transaction};

pub(crate) struct Message<'a> {
    pub id: &'a str,
    pub chat_id: &'a str,
    pub role: &'a str,
    pub text: &'a str,
    pub created_at: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub skill: Option<&'a str>,
    pub actual_effort: Option<&'a str>,
    pub finish_reason: Option<&'a str>,
}

pub(crate) fn insert(tx: &Transaction<'_>, message: &Message<'_>) -> rusqlite::Result<usize> {
    tx.execute(
        "INSERT INTO chat_messages(id,chat_id,role,text,created_at,provider,model,effort,skill,actual_effort,finish_reason) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![message.id, message.chat_id, message.role, message.text, message.created_at, message.provider, message.model, message.effort, message.skill, message.actual_effort, message.finish_reason],
    )
}
