//! Local-only cache of icons already obtained by the panel WebView.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::Manager;

const MAX_ICON_BYTES: usize = 64 * 1024;
const PNG_HEADER: &[u8] = b"\x89PNG\r\n\x1a\n";

pub(crate) fn cache_root(app: &tauri::AppHandle) -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("MOMSO_TEST_SAVED_LOGIN_ICON_ROOT") {
        return Some(PathBuf::from(path));
    }
    app.path().app_data_dir().ok()
}

fn icon_path(root: &Path, origin: &str) -> PathBuf {
    let digest = Sha256::digest(origin.as_bytes());
    root.join("saved-login-icons")
        .join(format!("{digest:x}.png"))
}

fn valid_png(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_ICON_BYTES && bytes.starts_with(PNG_HEADER)
}

#[cfg(any(windows, test))]
fn write_icon(root: &Path, origin: &str, bytes: &[u8]) -> std::io::Result<()> {
    if !valid_png(bytes) {
        return Ok(());
    }
    let path = icon_path(root, origin);
    let directory = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(directory)?;
    let temp = tempfile::NamedTempFile::new_in(directory)?;
    std::fs::write(temp.path(), bytes)?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub(crate) fn icons_for_origins<'a>(
    root: &Path,
    origins: impl Iterator<Item = &'a str>,
) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let mut seen = HashSet::new();
    for origin in origins {
        if !seen.insert(origin) {
            continue;
        }
        let bytes = std::fs::File::open(icon_path(root, origin)).and_then(|file| {
            let mut bytes = Vec::new();
            file.take((MAX_ICON_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        if let Ok(bytes) = bytes {
            if valid_png(&bytes) {
                result.insert(
                    origin.to_owned(),
                    format!("data:image/png;base64,{}", STANDARD.encode(bytes)),
                );
            }
        }
    }
    result
}

#[cfg(windows)]
pub(crate) fn observe(view: &tauri::Webview) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2_15, COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
    };
    use webview2_com::{FaviconChangedEventHandler, GetFaviconCompletedHandler};
    use windows_core::Interface;

    fn current_origin(core: &ICoreWebView2) -> Option<String> {
        let mut raw = windows_core::PWSTR::null();
        unsafe {
            core.Source(&mut raw).ok()?;
        }
        let source = webview2_com::CoTaskMemPWSTR::from(raw).to_string();
        let url = reqwest::Url::parse(&source).ok()?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return None;
        }
        Some(url.origin().ascii_serialization())
    }

    fn capture(core: ICoreWebView2, root: PathBuf) {
        let Some(origin) = current_origin(&core) else {
            return;
        };
        let Ok(core15) = core.cast::<ICoreWebView2_15>() else {
            return;
        };
        let handler = GetFaviconCompletedHandler::create(Box::new(move |status, stream| {
            if status.is_err() || current_origin(&core).as_deref() != Some(origin.as_str()) {
                return Ok(());
            }
            let Some(stream) = stream else {
                return Ok(());
            };
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let mut read = 0;
                if unsafe {
                    stream.Read(
                        chunk.as_mut_ptr().cast(),
                        chunk.len() as u32,
                        Some(&mut read),
                    )
                }
                .is_err()
                {
                    return Ok(());
                }
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..read as usize]);
                if bytes.len() > MAX_ICON_BYTES {
                    return Ok(());
                }
            }
            if valid_png(&bytes) {
                let _ = std::thread::spawn(move || {
                    let _ = write_icon(&root, &origin, &bytes);
                });
            }
            Ok(())
        }));
        let _ = unsafe { core15.GetFavicon(COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, &handler) };
    }

    let Some(root) = cache_root(view.app_handle()) else {
        return;
    };
    let _ = view.with_webview(move |platform| unsafe {
        let Ok(core) = platform.controller().CoreWebView2() else {
            return;
        };
        let Ok(core15) = core.cast::<ICoreWebView2_15>() else {
            return;
        };
        let event_root = root.clone();
        let handler = FaviconChangedEventHandler::create(Box::new(move |sender, _| {
            if let Some(core) = sender {
                capture(core, event_root.clone());
            }
            Ok(())
        }));
        let mut token = 0;
        let _ = core15.add_FaviconChanged(&handler, &mut token);
        capture(core, root);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_icons_are_local_deduplicated_and_missing_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let icon = [PNG_HEADER, &[1, 2, 3]].concat();
        write_icon(dir.path(), "https://one.example", &icon).unwrap();
        let icons = icons_for_origins(
            dir.path(),
            [
                "https://one.example",
                "https://one.example",
                "https://two.example",
            ]
            .into_iter(),
        );
        assert_eq!(icons.len(), 1);
        assert!(icons["https://one.example"].starts_with("data:image/png;base64,"));
        assert!(!icons.contains_key("https://two.example"));
    }

    #[test]
    fn invalid_and_oversized_icons_are_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        write_icon(dir.path(), "https://example.test", b"<svg />").unwrap();
        let huge = [PNG_HEADER, &vec![0; MAX_ICON_BYTES]].concat();
        write_icon(dir.path(), "https://example.test", &huge).unwrap();
        assert!(!icon_path(dir.path(), "https://example.test").exists());
    }
}
