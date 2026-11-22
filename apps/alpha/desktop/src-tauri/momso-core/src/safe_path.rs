//! Shared validation for user-relative archive paths.
use std::path::{Component, Path};

pub fn safe_relative(path: &Path) -> bool {
    let Some(raw) = path.to_str() else {
        return false;
    };
    if raw.is_empty() || raw.encode_utf16().count() > 32_000 {
        return false;
    }
    path.components().all(|component| {
        let Component::Normal(part) = component else {
            return false;
        };
        let Some(name) = part.to_str() else {
            return false;
        };
        if name.is_empty()
            || name.encode_utf16().count() > 255
            || name.ends_with(['.', ' '])
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return false;
        }
        let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) {
            return false;
        }
        let device_suffix = stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"));
        !device_suffix.is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

pub fn valid_document_reference(id: &str) -> bool {
    let Some(path) = id.strip_prefix("document-path:") else {
        return false;
    };
    !path.contains('\\')
        && !path.contains(':')
        && !path.chars().any(char::is_control)
        && safe_relative(Path::new(path))
        && Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("md"))
}
