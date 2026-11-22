//! Authenticated access to existing desktop-format work receipts.
use super::*;

fn head(value: momso_core::receipt_format::ReceiptHead) -> Value {
    json!({"id":value.id,"createdAt":value.created_at,"tool":value.tool,"model":value.model,"chatId":value.chat,"read":value.read,"wrote":value.wrote,"writeActions":value.write_actions,"evidence":value.evidence,"unknownActor":value.unknown_actor})
}
fn fail(reason: &'static str) -> ApiError {
    error(
        match reason {
            "invalid_id" => StatusCode::BAD_REQUEST,
            "not_found" => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        },
        reason,
    )
}
pub async fn list(
    State(app): State<App>,
    Query(query): Query<HashMap<String, String>>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let offset = query
        .get("offset")
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|_| fail("invalid_id"))?
        .unwrap_or(0);
    let limit = query
        .get("limit")
        .map(|v| v.parse::<usize>())
        .transpose()
        .map_err(|_| fail("invalid_id"))?
        .unwrap_or(50);
    if limit == 0 || limit > 100 {
        return Err(fail("invalid_id"));
    }
    let guard = ReadGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _guard=guard;
        let (items,unreadable)=momso_core::receipt_store::list(&app.data.join("archive")).map_err(fail)?;
        let total=items.len();
        let items:Vec<Value>=items.into_iter().skip(offset).take(limit).map(head).collect();
        Ok(Json(json!({"items":items,"total":total,"unreadable":unreadable,"offset":offset,"limit":limit})))
    }).await.map_err(|_|fail("storage_unavailable"))?
}
pub async fn read(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    auth(&app, &h)?;
    let guard = ReadGuard::new(&app);
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let (record, body) =
            momso_core::receipt_store::read(&app.data.join("archive"), &id).map_err(fail)?;
        Ok(Json(json!({"head":head(record),"body":body})))
    })
    .await
    .map_err(|_| fail("storage_unavailable"))?
}
