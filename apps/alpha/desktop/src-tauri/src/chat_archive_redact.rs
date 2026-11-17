//! Best-effort redaction for an exported copy, never for the local original.
use regex::Regex;
use std::sync::OnceLock;

pub(crate) struct Redacted {
    pub text: String,
    pub count: usize,
}

pub(crate) fn redact(input: &str) -> Redacted {
    redact_with_known(input, &[])
}

/// `known` must be supplied from the app's own credential store. Callers must
/// not include credentials owned by external tools such as GitHub CLI.
pub(crate) fn redact_with_known(input: &str, known: &[&str]) -> Redacted {
    static ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    static PRIVATE_KEY: OnceLock<Regex> = OnceLock::new();
    static BEARER: OnceLock<Regex> = OnceLock::new();
    let assignment = ASSIGNMENT.get_or_init(|| Regex::new(concat!(
            r#"(?i)\b((?:aws_secret_access_key|cloudflare_api_token|cloudflare_api_key|openai_api_key|anthropic_api_key|gemini_api_key|google_api_key|github_token)["']?\s*[:=]\s*["']?)"#,
        r#"([A-Za-z0-9_+./=-]{16,})"#
    )).unwrap());
    let token = TOKEN.get_or_init(|| {
        Regex::new(concat!(
            r"\b(?:sk-ant-[A-Za-z0-9_-]{16,}|sk-proj-[A-Za-z0-9_-]{16,}|sk-[A-Za-z0-9_-]{20,}",
            r"|AIza[A-Za-z0-9_-]{35}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}",
            r"|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16})"
        ))
        .unwrap()
    });
    let private_key = PRIVATE_KEY.get_or_init(|| Regex::new(
        r"(?s)-----BEGIN (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----.*?-----END (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----"
    ).unwrap());
    let bearer = BEARER.get_or_init(|| {
        Regex::new(r"(?i)\bauthorization\s*:\s*bearer\s+([A-Za-z0-9._~+/=-]+)").unwrap()
    });
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    for found in private_key.find_iter(input) {
        spans.push((
            found.start(),
            found.end(),
            "[비밀값으로 보여 가림: 개인 키]",
        ));
    }
    for found in token.find_iter(input) {
        spans.push((found.start(), found.end(), "[비밀값으로 보여 가림: API 키]"));
    }
    for capture in assignment.captures_iter(input) {
        if let Some(found) = capture.get(2) {
            spans.push((found.start(), found.end(), "[비밀값으로 보여 가림: API 키]"));
        }
    }
    for capture in bearer.captures_iter(input) {
        if let Some(found) = capture.get(1) {
            spans.push((found.start(), found.end(), "[비밀값으로 보여 가림: 토큰]"));
        }
    }
    for secret in known.iter().copied().filter(|s| !s.is_empty()) {
        let mut offset = 0;
        while offset < input.len() {
            let Some(found) = input[offset..].find(secret) else {
                break;
            };
            let start = offset + found;
            spans.push((
                start,
                start + secret.len(),
                "[비밀값으로 보여 가림: 저장된 값]",
            ));
            offset = start
                + input[start..]
                    .chars()
                    .next()
                    .expect("nonempty match")
                    .len_utf8();
        }
    }
    spans.sort_by_key(|span| (span.0, std::cmp::Reverse(span.1)));
    let mut merged: Vec<(usize, usize, &'static str)> = Vec::new();
    for (start, end, label) in spans {
        if let Some(last) = merged.last_mut() {
            if start < last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end, label));
    }
    let mut text = String::with_capacity(input.len());
    let mut cursor = 0;
    for (start, end, label) in &merged {
        text.push_str(&input[cursor..*start]);
        text.push_str(label);
        cursor = *end;
    }
    text.push_str(&input[cursor..]);
    Redacted {
        text,
        count: merged.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::{redact, redact_with_known};
    #[test]
    fn redacts_bearer_and_owned_values_without_partial_disclosure() {
        let bearer = "opaque-bearer-example-123456789";
        let owned = "사용자-보관값-시험";
        let input = format!("Authorization: Bearer {bearer}\n{owned} {owned}");
        let original = input.clone();
        let result = redact_with_known(&input, &[owned]);
        assert_eq!(result.count, 3);
        assert!(!result.text.contains(bearer));
        assert!(!result.text.contains(owned));
        assert_eq!(input, original);
        assert!(result
            .text
            .contains("Authorization: Bearer [비밀값으로 보여 가림: 토큰]"));
    }

    #[test]
    fn merges_overlapping_known_and_pattern_matches() {
        let secret = ["sk-proj-", &"z".repeat(32)].concat();
        let result = redact_with_known(&secret, &[&secret, &secret[8..]]);
        assert_eq!(result.count, 1);
        assert!(!result.text.contains(&"z".repeat(10)));
    }

    #[test]
    fn leaves_long_urls_hashes_and_lesson_text_untouched() {
        let input = format!(
            "수업 기록 https://example.invalid/{} {}",
            "a".repeat(80),
            "b".repeat(64)
        );
        let result = redact_with_known(&input, &["unrelated secret"]);
        assert_eq!(result.count, 0);
        assert_eq!(result.text, input);
    }
    #[test]
    fn redacts_known_forms_on_copy_only() {
        let sample = format!(
            "{} {}",
            ["sk-proj-", &"A".repeat(32)].concat(),
            ["ghp_", &"B".repeat(36)].concat()
        );
        let before = sample.clone();
        let result = redact(&sample);
        assert_eq!(result.count, 2);
        assert!(!result.text.contains(&"A".repeat(32)));
        assert!(!result.text.contains(&"B".repeat(36)));
        assert_eq!(sample, before);
    }
    #[test]
    fn redacts_named_cloud_secrets_without_masking_long_links() {
        let sample = format!(
            "CLOUDFLARE_API_TOKEN={} https://example.invalid/{}",
            "x".repeat(40),
            "y".repeat(80)
        );
        let result = redact(&sample);
        assert_eq!(result.count, 1);
        assert!(!result.text.contains(&"x".repeat(40)));
        assert!(result.text.contains(&"y".repeat(80)));
        let json = format!("{{\"aws_secret_access_key\":\"{}\"}}", "x".repeat(40));
        let result = redact(&json);
        assert_eq!(result.count, 1);
        assert!(!result.text.contains(&"x".repeat(40)));
    }
    #[test]
    fn redacts_supported_provider_and_private_key_shapes() {
        let anthropic = ["sk-ant-", &"a".repeat(30)].concat();
        let gemini = ["AIza", &"b".repeat(35)].concat();
        let aws = ["AKIA", &"C".repeat(16)].concat();
        let pem = [
            "-----BEGIN PRIVATE KEY-----\n",
            &"d".repeat(80),
            "\n-----END PRIVATE KEY-----",
        ]
        .concat();
        let input = format!("{anthropic} {gemini} {aws}\n{pem}");
        let result = redact(&input);
        assert_eq!(result.count, 4);
        for secret in [&anthropic, &gemini, &aws, &pem] {
            assert!(!result.text.contains(secret));
        }
    }
}
