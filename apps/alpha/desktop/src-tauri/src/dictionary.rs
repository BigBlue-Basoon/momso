use crate::storage;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub(crate) enum DictionaryError {
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
    #[error("전사 보정 사전을 읽거나 저장하지 못했습니다")]
    Sqlite(#[from] rusqlite::Error),
    #[error("잘못 들린 표현과 바꿀 표현을 모두 입력해 주세요")]
    MissingText,
    #[error("전사 보정 표현은 각각 100자 이하여야 합니다")]
    TextTooLong,
    #[error("메모는 300자 이하여야 합니다")]
    NoteTooLong,
    #[error("해당 보정 규칙을 찾을 수 없습니다")]
    NotFound,
    #[error("같은 표현을 가진 보정 규칙이 둘 이상이라 자동으로 바꿀 수 없습니다")]
    AmbiguousTerm,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranscriptionTerm {
    pub(crate) id: String,
    pub(crate) heard_as: String,
    pub(crate) corrected_to: String,
    pub(crate) note: Option<String>,
    pub(crate) enabled: bool,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Clone)]
pub(crate) enum DictionaryMutation {
    Upsert {
        heard_as: String,
        corrected_to: String,
        note: Option<String>,
    },
    SetEnabled {
        heard_as: String,
        enabled: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DictionaryMutationResult {
    Upserted {
        heard_as: String,
        corrected_to: String,
        created: bool,
    },
    SetEnabled {
        heard_as: String,
        enabled: bool,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateTranscriptionTermInput {
    heard_as: String,
    corrected_to: String,
    note: Option<String>,
}

fn read_term(row: &rusqlite::Row<'_>) -> rusqlite::Result<TranscriptionTerm> {
    Ok(TranscriptionTerm {
        id: row.get(0)?,
        heard_as: row.get(1)?,
        corrected_to: row.get(2)?,
        note: row.get(3)?,
        enabled: row.get::<_, i64>(4)? == 1,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub(crate) fn list(database_path: &Path) -> Result<Vec<TranscriptionTerm>, DictionaryError> {
    let connection = storage::open_writable_connection(database_path)?;
    let mut statement = connection.prepare(
        "SELECT id, heard_as, corrected_to, note, enabled, created_at, updated_at
         FROM momso_transcription_terms
         ORDER BY enabled DESC, heard_as COLLATE NOCASE ASC, created_at ASC",
    )?;
    let rows = statement
        .query_map([], read_term)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn enabled_prompt_entries(
    database_path: &Path,
) -> Result<Vec<(String, String)>, DictionaryError> {
    let connection = storage::open_writable_connection(database_path)?;
    let mut statement = connection.prepare(
        "SELECT heard_as, corrected_to
         FROM momso_transcription_terms
         WHERE enabled = 1
         ORDER BY heard_as COLLATE NOCASE ASC, corrected_to COLLATE NOCASE ASC",
    )?;
    let entries = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DictionaryError::from)?;
    Ok(entries)
}

pub(crate) fn create(
    database_path: &Path,
    input: CreateTranscriptionTermInput,
) -> Result<TranscriptionTerm, DictionaryError> {
    let heard_as = input.heard_as.trim();
    let corrected_to = input.corrected_to.trim();
    let note = input
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if heard_as.is_empty() || corrected_to.is_empty() {
        return Err(DictionaryError::MissingText);
    }
    if heard_as.chars().count() > 100 || corrected_to.chars().count() > 100 {
        return Err(DictionaryError::TextTooLong);
    }
    if note
        .map(|value| value.chars().count() > 300)
        .unwrap_or(false)
    {
        return Err(DictionaryError::NoteTooLong);
    }

    let connection = storage::open_writable_connection(database_path)?;
    let id = Uuid::new_v4().to_string();
    connection.execute(
        "INSERT INTO momso_transcription_terms (
           id, heard_as, corrected_to, note, enabled, created_at, updated_at
         ) VALUES (
           ?1, ?2, ?3, ?4, 1,
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
           strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         )",
        (&id, heard_as, corrected_to, note),
    )?;
    get(&connection, &id)?.ok_or(DictionaryError::NotFound)
}

pub(crate) fn set_enabled(
    database_path: &Path,
    id: &str,
    enabled: bool,
) -> Result<TranscriptionTerm, DictionaryError> {
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_transcription_terms
         SET enabled = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        (id, i64::from(enabled)),
    )?;
    if changed == 0 {
        return Err(DictionaryError::NotFound);
    }
    get(&connection, id)?.ok_or(DictionaryError::NotFound)
}

pub(crate) fn delete(database_path: &Path, id: &str) -> Result<(), DictionaryError> {
    let connection = storage::open_writable_connection(database_path)?;
    let changed = connection.execute(
        "UPDATE momso_transcription_terms
         SET enabled = 0, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         WHERE id = ?1",
        [id],
    )?;
    if changed == 0 {
        return Err(DictionaryError::NotFound);
    }
    Ok(())
}

pub(crate) fn apply_mutations(
    database_path: &Path,
    mutations: Vec<DictionaryMutation>,
) -> Result<Vec<DictionaryMutationResult>, DictionaryError> {
    let mut connection = storage::open_writable_connection(database_path)?;
    let transaction = connection.transaction()?;
    let mut results = Vec::with_capacity(mutations.len());

    for mutation in mutations {
        match mutation {
            DictionaryMutation::Upsert {
                heard_as,
                corrected_to,
                note,
            } => {
                let (heard_as, corrected_to, note) =
                    normalize_term(&heard_as, &corrected_to, note.as_deref())?;
                let existing_id = find_ids_by_heard_as(&transaction, &heard_as)?;
                let created = match existing_id.as_slice() {
                    [] => {
                        let id = Uuid::new_v4().to_string();
                        transaction.execute(
                            "INSERT INTO momso_transcription_terms (
                               id, heard_as, corrected_to, note, enabled, created_at, updated_at
                             ) VALUES (
                               ?1, ?2, ?3, ?4, 1,
                               strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                               strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             )",
                            (&id, &heard_as, &corrected_to, note.as_deref()),
                        )?;
                        true
                    }
                    [id] => {
                        transaction.execute(
                            "UPDATE momso_transcription_terms
                             SET corrected_to = ?2,
                                 note = COALESCE(?3, note),
                                 enabled = 1,
                                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                             WHERE id = ?1",
                            (id, &corrected_to, note.as_deref()),
                        )?;
                        false
                    }
                    _ => return Err(DictionaryError::AmbiguousTerm),
                };
                results.push(DictionaryMutationResult::Upserted {
                    heard_as,
                    corrected_to,
                    created,
                });
            }
            DictionaryMutation::SetEnabled { heard_as, enabled } => {
                let heard_as = normalize_reference(&heard_as)?;
                let ids = find_ids_by_heard_as(&transaction, &heard_as)?;
                let [id] = ids.as_slice() else {
                    return if ids.is_empty() {
                        Err(DictionaryError::NotFound)
                    } else {
                        Err(DictionaryError::AmbiguousTerm)
                    };
                };
                transaction.execute(
                    "UPDATE momso_transcription_terms
                     SET enabled = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     WHERE id = ?1",
                    (id, i64::from(enabled)),
                )?;
                results.push(DictionaryMutationResult::SetEnabled { heard_as, enabled });
            }
        }
    }

    transaction.commit()?;
    Ok(results)
}

fn normalize_term(
    heard_as: &str,
    corrected_to: &str,
    note: Option<&str>,
) -> Result<(String, String, Option<String>), DictionaryError> {
    let heard_as = normalize_reference(heard_as)?;
    let corrected_to = corrected_to.trim().to_owned();
    if corrected_to.is_empty() {
        return Err(DictionaryError::MissingText);
    }
    if corrected_to.chars().count() > 100 {
        return Err(DictionaryError::TextTooLong);
    }
    let note = note
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if note
        .as_ref()
        .map(|value| value.chars().count() > 300)
        .unwrap_or(false)
    {
        return Err(DictionaryError::NoteTooLong);
    }
    Ok((heard_as, corrected_to, note))
}

fn normalize_reference(value: &str) -> Result<String, DictionaryError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(DictionaryError::MissingText);
    }
    if value.chars().count() > 100 {
        return Err(DictionaryError::TextTooLong);
    }
    Ok(value.to_owned())
}

fn find_ids_by_heard_as(
    connection: &rusqlite::Connection,
    heard_as: &str,
) -> Result<Vec<String>, DictionaryError> {
    let mut statement = connection.prepare(
        "SELECT id FROM momso_transcription_terms
         WHERE heard_as = ?1 COLLATE NOCASE
         ORDER BY created_at ASC",
    )?;
    let ids = statement
        .query_map([heard_as], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DictionaryError::from)?;

    Ok(ids)
}

fn get(
    connection: &rusqlite::Connection,
    id: &str,
) -> Result<Option<TranscriptionTerm>, DictionaryError> {
    connection
        .query_row(
            "SELECT id, heard_as, corrected_to, note, enabled, created_at, updated_at
             FROM momso_transcription_terms WHERE id = ?1",
            [id],
            read_term,
        )
        .optional()
        .map_err(DictionaryError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempdir().expect("tempdir");
        let database_path = temp.path().join("momso.db");
        storage::initialize(&database_path, &temp.path().join("backups"), "test")
            .expect("storage initializes");
        (temp, database_path)
    }

    #[test]
    fn creates_lists_disables_and_retires_a_term() {
        let (_temp, database_path) = database();
        let term = create(
            &database_path,
            CreateTranscriptionTermInput {
                heard_as: "아사나아".into(),
                corrected_to: "아사나".into(),
                note: Some("요가 용어".into()),
            },
        )
        .expect("term creates");
        assert_eq!(list(&database_path).expect("terms list").len(), 1);
        assert!(
            !set_enabled(&database_path, &term.id, false)
                .expect("term disables")
                .enabled
        );
        delete(&database_path, &term.id).expect("term retires");
        let terms = list(&database_path).expect("terms list");
        assert_eq!(terms.len(), 1);
        assert!(!terms[0].enabled);
    }

    #[test]
    fn skill_mutations_upsert_toggle_and_retire_by_spoken_form() {
        let (_temp, database_path) = database();
        let created = apply_mutations(
            &database_path,
            vec![DictionaryMutation::Upsert {
                heard_as: "바순".into(),
                corrected_to: "파순".into(),
                note: None,
            }],
        )
        .expect("skill creates");
        assert_eq!(
            created,
            vec![DictionaryMutationResult::Upserted {
                heard_as: "바순".into(),
                corrected_to: "파순".into(),
                created: true,
            }]
        );

        let updated = apply_mutations(
            &database_path,
            vec![DictionaryMutation::Upsert {
                heard_as: "바순".into(),
                corrected_to: "파스춘".into(),
                note: None,
            }],
        )
        .expect("skill updates");
        assert!(matches!(
            updated.as_slice(),
            [DictionaryMutationResult::Upserted { created: false, .. }]
        ));
        assert_eq!(list(&database_path).expect("terms list").len(), 1);

        apply_mutations(
            &database_path,
            vec![DictionaryMutation::SetEnabled {
                heard_as: "바순".into(),
                enabled: false,
            }],
        )
        .expect("skill disables");
        assert!(!list(&database_path).expect("terms list")[0].enabled);

        apply_mutations(
            &database_path,
            vec![DictionaryMutation::SetEnabled {
                heard_as: "바순".into(),
                enabled: false,
            }],
        )
        .expect("skill retires");
        let terms = list(&database_path).expect("terms list");
        assert_eq!(terms.len(), 1);
        assert!(!terms[0].enabled);
    }

    #[test]
    fn skill_mutations_roll_back_as_one_change_when_any_action_fails() {
        let (_temp, database_path) = database();
        let error = apply_mutations(
            &database_path,
            vec![
                DictionaryMutation::Upsert {
                    heard_as: "바순".into(),
                    corrected_to: "파순".into(),
                    note: None,
                },
                DictionaryMutation::SetEnabled {
                    heard_as: "없는 용어".into(),
                    enabled: false,
                },
            ],
        )
        .expect_err("a missing target rejects the whole change");

        assert!(matches!(error, DictionaryError::NotFound));
        assert!(list(&database_path).expect("terms list").is_empty());
    }
}
