//! Chat groups are stored in the server database and backed up with chat copies.
use super::*;
use std::collections::HashSet;

fn valid_id(id: &str) -> bool {
    Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 200 && !name.chars().any(char::is_control)
}

pub async fn list(State(app): State<App>, h: HeaderMap) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let conn = db(&app)?;
    let mut stmt = conn.prepare("SELECT id,name,position,created_at,(SELECT count(*) FROM chats WHERE group_id=chat_groups.id AND archived_at IS NULL) FROM chat_groups WHERE deleted_at IS NULL ORDER BY position,id").map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"storage_error"))?;
    let rows = stmt.query_map([], |r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"order":r.get::<_,i64>(2)?,"createdAt":r.get::<_,String>(3)?,"chatCount":r.get::<_,i64>(4)?}))).map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"storage_error"))?;
    let groups = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    Ok(Json(json!({"groups":groups})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Name {
    name: String,
}

pub async fn create(
    State(app): State<App>,
    h: HeaderMap,
    Json(body): Json<Name>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    if !valid_name(&body.name) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_group_name"));
    }
    let _guard = WorkGuard::new(&app);
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    db(&app)?.execute("INSERT INTO chat_groups(id,name,position,created_at) VALUES(?1,?2,(SELECT COALESCE(MAX(position),-1)+1 FROM chat_groups WHERE deleted_at IS NULL),?3)",params![id,body.name.trim(),now]).map_err(|_|error(StatusCode::INTERNAL_SERVER_ERROR,"storage_error"))?;
    Ok(Json(
        json!({"id":id,"name":body.name.trim(),"createdAt":now}),
    ))
}

pub async fn rename(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(body): Json<Name>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    if !valid_id(&id) || !valid_name(&body.name) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_arguments"));
    }
    let _guard = WorkGuard::new(&app);
    let count = db(&app)?
        .execute(
            "UPDATE chat_groups SET name=?2 WHERE id=?1 AND deleted_at IS NULL",
            params![id, body.name.trim()],
        )
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    if count == 0 {
        return Err(error(StatusCode::NOT_FOUND, "group_not_found"));
    }
    Ok(Json(json!({"id":id,"name":body.name.trim()})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Order {
    ids: Vec<String>,
}

pub async fn reorder(
    State(app): State<App>,
    h: HeaderMap,
    Json(body): Json<Order>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let unique: HashSet<&str> = body.ids.iter().map(String::as_str).collect();
    if unique.len() != body.ids.len()
        || body.ids.len() > 10000
        || body.ids.iter().any(|id| !valid_id(id))
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_arguments"));
    }
    let _guard = WorkGuard::new(&app);
    let mut conn = db(&app)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    let count: i64 = tx
        .query_row(
            "SELECT count(*) FROM chat_groups WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    if count as usize != body.ids.len() {
        return Err(error(StatusCode::CONFLICT, "group_order_changed"));
    }
    for (position, id) in body.ids.iter().enumerate() {
        let changed = tx
            .execute(
                "UPDATE chat_groups SET position=?2 WHERE id=?1 AND deleted_at IS NULL",
                params![id, position as i64],
            )
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
        if changed != 1 {
            return Err(error(StatusCode::CONFLICT, "group_order_changed"));
        }
    }
    tx.commit()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    Ok(Json(json!({"ids":body.ids})))
}

pub async fn remove(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    if !valid_id(&id) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_arguments"));
    }
    let _guard = WorkGuard::new(&app);
    let mut conn = db(&app)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    let now = chrono::Utc::now().to_rfc3339();
    let changed = tx
        .execute(
            "UPDATE chat_groups SET name='',deleted_at=?2 WHERE id=?1 AND deleted_at IS NULL",
            params![id, now],
        )
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    if changed == 0 {
        return Err(error(StatusCode::NOT_FOUND, "group_not_found"));
    }
    tx.execute("UPDATE chats SET group_id=NULL WHERE group_id=?1", [&id])
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    tx.commit()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    Ok(Json(json!({"id":id,"deletedAt":now})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Assignment {
    group_id: Option<String>,
}

pub async fn assign(
    State(app): State<App>,
    Path(chat): Path<String>,
    h: HeaderMap,
    Json(body): Json<Assignment>,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    if body.group_id.as_ref().is_some_and(|id| !valid_id(id)) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid_arguments"));
    }
    let _guard = WorkGuard::new(&app);
    let mut conn = db(&app)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    if let Some(id) = &body.group_id {
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_groups WHERE id=?1 AND deleted_at IS NULL)",
                [id],
                |r| r.get(0),
            )
            .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
        if !exists {
            return Err(error(StatusCode::NOT_FOUND, "group_not_found"));
        }
    }
    let count = tx
        .execute(
            "UPDATE chats SET group_id=?2,updated_at=?3 WHERE id=?1",
            params![chat, body.group_id, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    if count == 0 {
        return Err(error(StatusCode::NOT_FOUND, "chat_not_found"));
    }
    tx.commit()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_error"))?;
    Ok(Json(json!({"chatId":chat,"groupId":body.group_id})))
}
