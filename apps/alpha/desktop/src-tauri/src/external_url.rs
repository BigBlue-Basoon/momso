//! Open only ordinary web addresses; never connect to local or credentialed URLs.
use tauri_plugin_opener::OpenerExt;

fn validate(address: &str) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(address).map_err(|_| "invalid_url")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || !crate::object_store::public_literal_host(&url)
    {
        return Err("invalid_url");
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn open_external_url(app: tauri::AppHandle, url: String) -> Result<(), String> {
    validate(&url).map_err(str::to_owned)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|_| "unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normal_web_links_work_and_local_or_credentialed_urls_are_rejected() {
        for url in [
            "https://example.test/document?q=one",
            "http://example.test/chat",
        ] {
            assert!(validate(url).is_ok());
        }
        for url in [
            "file:///C:/fixture",
            "javascript:alert(1)",
            "https://localhost/",
            "http://127.0.0.1/",
            "http://127.1/",
            "http://[::1]/",
            "https://169.254.169.254/",
            "https://10.1.2.3/",
            "https://172.16.1.1/",
            "https://192.168.1.1/",
            "https://[fd00::1]/",
            "https://person@example.com/",
            "https://person:pass@example.com/",
        ] {
            assert!(validate(url).is_err(), "{url}");
        }
    }
}
