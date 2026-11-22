//! Extract only values the provider actually reported.
pub fn codex_last_usage(params: &serde_json::Value) -> Option<(i64, Option<i64>)> {
    let used = params
        .pointer("/tokenUsage/last/totalTokens")?
        .as_i64()
        .filter(|v| *v >= 0)?;
    let limit = params
        .pointer("/tokenUsage/modelContextWindow")
        .and_then(serde_json::Value::as_i64)
        .filter(|v| *v > 0);
    Some((used, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_or_invalid_usage_is_not_invented() {
        assert_eq!(codex_last_usage(&serde_json::json!({})), None);
        assert_eq!(
            codex_last_usage(&serde_json::json!({"tokenUsage":{"last":{"totalTokens":-1}}})),
            None
        );
        assert_eq!(
            codex_last_usage(
                &serde_json::json!({"tokenUsage":{"last":{"totalTokens":7},"modelContextWindow":0}})
            ),
            Some((7, None))
        );
    }
}
