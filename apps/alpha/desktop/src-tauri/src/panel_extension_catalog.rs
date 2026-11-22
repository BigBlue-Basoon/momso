//! User-selected browser extensions and profile-wide toolbar preferences.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use tauri::{Emitter, Manager};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Catalog {
    paths: BTreeMap<String, PathBuf>,
    pinned: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Candidate {
    browser: String,
    profile: String,
    extension_id: String,
    version: String,
    name: String,
}

static CATALOG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// WebView2's extension object exposes only ID, name and enabled state. These
// two IDs are documented in the WebView2 feedback example; all other IDs
// without an install record remain unknown rather than being called built in.
const BUILT_IN_IDS: &[&str] = &[
    "dgiklkfkllikcanfonkcabmbdfmgleag",
    "mhjfbmdgcfjbbpaeojofohoefgiehjai",
];

struct CachedIcon {
    manifest_size: u64,
    manifest_modified: std::time::SystemTime,
    icon_path: PathBuf,
    icon_size: u64,
    icon_modified: std::time::SystemTime,
    created: std::time::Instant,
    data: String,
}

fn icon_cache() -> &'static std::sync::Mutex<BTreeMap<PathBuf, CachedIcon>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<PathBuf, CachedIcon>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

fn bounded_read(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut data = Vec::new();
    file.take(limit + 1).read_to_end(&mut data).ok()?;
    (data.len() as u64 <= limit).then_some(data)
}

fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.starts_with(b"\x00\x00\x01\x00") {
        Some("image/x-icon")
    } else {
        None
    }
}

fn icon_data(folder: &Path) -> Option<String> {
    use base64::Engine as _;
    let root = folder.canonicalize().ok()?;
    let manifest = root.join("manifest.json").canonicalize().ok()?;
    if !manifest.starts_with(&root) {
        return None;
    }
    let m = std::fs::metadata(&manifest).ok()?;
    if !m.is_file() || m.len() > 512 * 1024 {
        return None;
    }
    let modified = m.modified().ok()?;
    let cached = icon_cache().lock().ok().and_then(|cache| {
        cache
            .get(&root)
            .filter(|entry| {
                entry.manifest_size == m.len()
                    && entry.manifest_modified == modified
                    && entry.created.elapsed() < std::time::Duration::from_secs(30)
                    && std::fs::metadata(&entry.icon_path).is_ok_and(|meta| {
                        meta.is_file()
                            && meta.len() == entry.icon_size
                            && meta.modified().ok() == Some(entry.icon_modified)
                    })
            })
            .map(|entry| entry.data.clone())
    });
    if cached.is_some() {
        return cached;
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bounded_read(&manifest, 512 * 1024)?).ok()?;
    let mut candidates = Vec::<(u32, String)>::new();
    for icon in [
        value.get("icons"),
        value.pointer("/action/default_icon"),
        value.pointer("/browser_action/default_icon"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = icon.as_str() {
            candidates.push((32, path.to_owned()));
        }
        if let Some(map) = icon.as_object() {
            for (size, path) in map {
                if let (Some(size), Some(path)) = (
                    size.parse::<u32>()
                        .ok()
                        .filter(|size| (1..=4096).contains(size)),
                    path.as_str(),
                ) {
                    candidates.push((size, path.to_owned()));
                }
            }
        }
    }
    // Prefer the smallest icon at least 48px; otherwise the largest below it.
    candidates.sort_by_key(|(size, _)| {
        if *size >= 48 {
            (0u8, *size - 48)
        } else {
            (1u8, 48 - *size)
        }
    });
    for (_, path) in candidates {
        // Chrome treats a leading slash as the extension root, not the host's
        // filesystem root; retain all other traversal checks below.
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
        {
            continue;
        }
        let Ok(icon) = root.join(relative).canonicalize() else {
            continue;
        };
        if !icon.starts_with(&root) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&icon) else {
            continue;
        };
        if !meta.is_file() || meta.len() > 256 * 1024 {
            continue;
        }
        let Some(bytes) = bounded_read(&icon, 256 * 1024) else {
            continue;
        };
        let Some(mime) = image_mime(&bytes) else {
            continue;
        };
        let result = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        if let (Ok(icon_modified), Ok(mut cache)) = (meta.modified(), icon_cache().lock()) {
            cache.remove(&root);
            while cache.len() >= 128
                || cache.values().map(|entry| entry.data.len()).sum::<usize>() + result.len()
                    > 8 * 1024 * 1024
            {
                let oldest = cache
                    .iter()
                    .min_by_key(|(_, entry)| entry.created)
                    .map(|(path, _)| path.clone());
                let Some(oldest) = oldest else { break };
                cache.remove(&oldest);
            }
            cache.insert(
                root.clone(),
                CachedIcon {
                    manifest_size: m.len(),
                    manifest_modified: modified,
                    icon_path: icon,
                    icon_size: meta.len(),
                    icon_modified,
                    created: std::time::Instant::now(),
                    data: result.clone(),
                },
            );
        }
        return Some(result);
    }
    None
}

fn enrich_with_paths(
    mut items: Vec<crate::panel_extensions::ExtensionInfo>,
    paths: &BTreeMap<String, PathBuf>,
    install_path: Option<&Path>,
) -> Vec<crate::panel_extensions::ExtensionInfo> {
    let installing_one = items.len() == 1;
    for item in &mut items {
        if BUILT_IN_IDS.contains(&item.id.as_str()) {
            item.origin = "builtin";
            continue;
        }
        let selected = if installing_one { install_path } else { None };
        if let Some(path) = selected.or_else(|| paths.get(&item.id).map(PathBuf::as_path)) {
            item.origin = "user";
            item.icon_data_url = icon_data(path);
        }
    }
    items
}

pub(crate) fn enrich(
    app: &tauri::AppHandle,
    items: Vec<crate::panel_extensions::ExtensionInfo>,
    install_path: Option<&Path>,
) -> Vec<crate::panel_extensions::ExtensionInfo> {
    let paths = CATALOG_LOCK
        .lock()
        .ok()
        .and_then(|_guard| {
            catalog_path(app)
                .ok()
                .and_then(|p| load(&p).ok())
                .map(|c| c.paths)
        })
        .unwrap_or_default();
    enrich_with_paths(items, &paths, install_path)
}

fn catalog_path(app: &tauri::AppHandle) -> Result<PathBuf, &'static str> {
    Ok(extension_data_root(app)?.join("panel-extensions.json"))
}

pub(crate) fn extension_data_root(app: &tauri::AppHandle) -> Result<PathBuf, &'static str> {
    if let Some(state) = app.try_state::<crate::AppState>() {
        return Ok(state.paths.data_root.clone());
    }
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    if app
        .config()
        .identifier
        .starts_with("app.momso.s5b.fixture.")
    {
        let root = PathBuf::from(std::env::var_os("MOMSO_S5B_ROOT").ok_or("unavailable")?);
        if !root.is_absolute() || !root.starts_with(std::env::temp_dir()) {
            return Err("unavailable");
        }
        return Ok(root);
    }
    #[cfg(test)]
    if let Some(root) = std::env::var_os("MOMSO_CLICK_ROOT") {
        return Ok(PathBuf::from(root));
    }
    Err("unavailable")
}

fn load(path: &Path) -> Result<Catalog, &'static str> {
    match std::fs::read(path) {
        Ok(data) => serde_json::from_slice(&data).map_err(|_| "catalog_invalid"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Catalog::default()),
        Err(_) => Err("unavailable"),
    }
}

fn save(path: &Path, catalog: &Catalog) -> Result<(), &'static str> {
    let parent = path.parent().ok_or("unavailable")?;
    std::fs::create_dir_all(parent).map_err(|_| "unavailable")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|_| "unavailable")?;
    serde_json::to_writer(&mut file, catalog).map_err(|_| "unavailable")?;
    file.flush().map_err(|_| "unavailable")?;
    file.persist(path).map_err(|_| "unavailable")?;
    Ok(())
}

pub(crate) fn remember_path(
    app: &tauri::AppHandle,
    id: &str,
    path: &Path,
) -> Result<(), &'static str> {
    let _guard = CATALOG_LOCK.lock().map_err(|_| "unavailable")?;
    let file = catalog_path(app)?;
    let mut catalog = load(&file)?;
    catalog.paths.insert(id.to_owned(), path.to_path_buf());
    save(&file, &catalog)
}

pub(crate) fn forget(app: &tauri::AppHandle, id: &str) -> Result<(), &'static str> {
    let _guard = CATALOG_LOCK.lock().map_err(|_| "unavailable")?;
    let file = catalog_path(app)?;
    let mut catalog = load(&file)?;
    catalog.paths.remove(id);
    catalog.pinned.retain(|pinned| pinned != id);
    save(&file, &catalog)
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

fn one_component(name: &str) -> bool {
    matches!(
        (
            Path::new(name).components().next(),
            Path::new(name).components().nth(1)
        ),
        (Some(Component::Normal(_)), None)
    )
}

fn popup_url(id: &str, popup: &str) -> Result<String, &'static str> {
    let base =
        reqwest::Url::parse(&format!("chrome-extension://{id}/")).map_err(|_| "invalid_popup")?;
    let url = base.join(popup).map_err(|_| "invalid_popup")?;
    if url.scheme() != "chrome-extension" || url.host_str() != Some(id) {
        return Err("invalid_popup");
    }
    Ok(url.to_string())
}

fn extension_name(folder: &Path, manifest: &serde_json::Value, fallback: &str) -> String {
    let name = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback);
    let Some(key) = name
        .strip_prefix("__MSG_")
        .and_then(|s| s.strip_suffix("__"))
    else {
        return name.to_owned();
    };
    let default_locale = manifest
        .get("default_locale")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("en");
    for locale in ["ko", default_locale] {
        if !one_component(locale) {
            continue;
        }
        let Ok(bytes) = std::fs::read(folder.join("_locales").join(locale).join("messages.json"))
        else {
            continue;
        };
        let Ok(messages) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(text) = messages
            .get(key)
            .and_then(|entry| entry.get("message"))
            .and_then(serde_json::Value::as_str)
        {
            return text.to_owned();
        }
    }
    fallback.to_owned()
}

fn scan(browser: &str, root: &Path) -> Vec<Candidate> {
    let mut result = Vec::new();
    let Ok(profiles) = std::fs::read_dir(root) else {
        return result;
    };
    for profile in profiles.flatten() {
        let Some(profile_name) = profile.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(ids) = std::fs::read_dir(profile.path().join("Extensions")) else {
            continue;
        };
        for id in ids.flatten() {
            let Some(id_name) = id.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_id(&id_name) {
                continue;
            }
            let Ok(versions) = std::fs::read_dir(id.path()) else {
                continue;
            };
            for version in versions.flatten() {
                let Some(version_name) = version.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let Ok(bytes) = std::fs::read(version.path().join("manifest.json")) else {
                    continue;
                };
                let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    continue;
                };
                let name = extension_name(&version.path(), &manifest, &id_name);
                result.push(Candidate {
                    browser: browser.into(),
                    profile: profile_name.clone(),
                    extension_id: id_name.clone(),
                    version: version_name,
                    name,
                });
            }
        }
    }
    result.sort_by(|a, b| {
        (&a.browser, &a.profile, &a.extension_id, &a.version).cmp(&(
            &b.browser,
            &b.profile,
            &b.extension_id,
            &b.version,
        ))
    });
    result
}

fn browser_root(browser: &str, local: &Path) -> Result<PathBuf, &'static str> {
    match browser {
        "chrome" => Ok(local.join("Google/Chrome/User Data")),
        "edge" => Ok(local.join("Microsoft/Edge/User Data")),
        _ => Err("invalid_source"),
    }
}

fn candidate_path(
    root: &Path,
    profile: &str,
    id: &str,
    version: &str,
) -> Result<PathBuf, &'static str> {
    if !one_component(profile) || !valid_id(id) || !one_component(version) {
        return Err("invalid_source");
    }
    let base = root.canonicalize().map_err(|_| "source_unavailable")?;
    let source = root
        .join(profile)
        .join("Extensions")
        .join(id)
        .join(version)
        .canonicalize()
        .map_err(|_| "source_unavailable")?;
    if !source.starts_with(base) {
        return Err("invalid_source");
    }
    Ok(source)
}

fn copy_extension(source: &Path, destination: &Path) -> Result<(), &'static str> {
    for item in walkdir::WalkDir::new(source).follow_links(false) {
        let item = item.map_err(|_| "source_unavailable")?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let metadata = item.metadata().map_err(|_| "source_unavailable")?;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("invalid_source");
            }
        }
        if item.file_type().is_symlink() {
            return Err("invalid_source");
        }
        let relative = item
            .path()
            .strip_prefix(source)
            .map_err(|_| "invalid_source")?;
        let target = destination.join(relative);
        if item.file_type().is_dir() {
            std::fs::create_dir_all(target).map_err(|_| "unavailable")?;
        } else if item.file_type().is_file() {
            std::fs::copy(item.path(), target).map_err(|_| "unavailable")?;
        } else {
            return Err("invalid_source");
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn panel_extensions_import_candidates() -> Result<Vec<Candidate>, &'static str> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("unavailable")?;
    let mut candidates = scan("chrome", &browser_root("chrome", Path::new(&local))?);
    candidates.extend(scan("edge", &browser_root("edge", Path::new(&local))?));
    Ok(candidates)
}

#[tauri::command]
pub(crate) async fn panel_extensions_import(
    app: tauri::AppHandle,
    browser: String,
    profile: String,
    extension_id: String,
    version: String,
) -> Result<Vec<crate::panel_extensions::ExtensionInfo>, &'static str> {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA").ok_or("unavailable")?;
        let source = candidate_path(
            &browser_root(&browser, Path::new(&local))?,
            &profile,
            &extension_id,
            &version,
        )?;
        let parent = extension_data_root(&app)?.join("panel-extension-files");
        std::fs::create_dir_all(&parent).map_err(|_| "unavailable")?;
        let staging = tempfile::Builder::new()
            .prefix("import-")
            .tempdir_in(&parent)
            .map_err(|_| "unavailable")?;
        copy_extension(&source, staging.path())?;
        let folder = staging.keep();
        let installed = crate::panel_extensions::perform(
            &app,
            crate::panel_extensions::Operation::Install(folder.clone()),
        )
        .await?;
        let id = &installed.first().ok_or("install_failed")?.id;
        if remember_path(&app, id, &folder).is_err() {
            let _ = app.emit("panel-extensions-changed", ());
            return Err("installed_catalog_unavailable");
        }
        let _ = app.emit("panel-extensions-changed", ());
        Ok(installed)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, browser, profile, extension_id, version);
        Err("unsupported")
    }
}

#[tauri::command]
pub(crate) fn panel_extensions_pin_get(app: tauri::AppHandle) -> Result<Vec<String>, &'static str> {
    let _guard = CATALOG_LOCK.lock().map_err(|_| "unavailable")?;
    Ok(load(&catalog_path(&app)?)?.pinned)
}

#[tauri::command]
pub(crate) async fn panel_extensions_pin_set(
    app: tauri::AppHandle,
    extension_id: String,
    pinned: bool,
) -> Result<(), &'static str> {
    if !valid_id(&extension_id) {
        return Err("not_found");
    }
    let installed = crate::panel_extensions::panel_extensions_list(app.clone()).await?;
    if !installed.iter().any(|item| item.id == extension_id) {
        return Err("not_found");
    }
    {
        let _guard = CATALOG_LOCK.lock().map_err(|_| "unavailable")?;
        let path = catalog_path(&app)?;
        let mut catalog = load(&path)?;
        catalog.pinned.retain(|id| id != &extension_id);
        if pinned {
            catalog.pinned.push(extension_id);
        }
        save(&path, &catalog)?;
    }
    let _ = app.emit("panel-extensions-changed", ());
    Ok(())
}

#[tauri::command]
pub(crate) async fn panel_extensions_popup(
    app: tauri::AppHandle,
    extension_id: String,
) -> Result<serde_json::Value, &'static str> {
    if !valid_id(&extension_id) {
        return Err("not_found");
    }
    let installed = crate::panel_extensions::panel_extensions_list(app.clone()).await?;
    if !installed
        .iter()
        .any(|item| item.id == extension_id && item.enabled)
    {
        return Err("not_found");
    }
    let path = {
        let _guard = CATALOG_LOCK.lock().map_err(|_| "unavailable")?;
        load(&catalog_path(&app)?)?
            .paths
            .get(&extension_id)
            .cloned()
            .ok_or("manifest_unavailable")?
    };
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path.join("manifest.json")).map_err(|_| "manifest_unavailable")?,
    )
    .map_err(|_| "manifest_unavailable")?;
    let popup = manifest
        .pointer("/action/default_popup")
        .or_else(|| manifest.pointer("/browser_action/default_popup"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("no_popup")?;
    let url = popup_url(&extension_id, popup)?;
    Ok(serde_json::json!({"extensionId":extension_id,"url":url,"supported":true}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_icons_use_manifest_and_known_origins_without_guessing() {
        use base64::Engine as _;
        let root = tempfile::tempdir().unwrap();
        let small = b"\x89PNG\r\n\x1a\nsmall";
        let medium = base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/aXkAAAAASUVORK5CYII=").unwrap();
        std::fs::write(root.path().join("small.png"), small).unwrap();
        std::fs::write(root.path().join("medium.png"), &medium).unwrap();
        std::fs::write(
            root.path().join("manifest.json"),
            r#"{"icons":{"16":"small.png","48":"/medium.png"}}"#,
        )
        .unwrap();
        let extension_id = "abcdefghijklmnopabcdefghijklmnop";
        let entries = vec![
            crate::panel_extensions::ExtensionInfo {
                id: extension_id.into(),
                name: "fixture".into(),
                enabled: true,
                icon_data_url: None,
                origin: "unknown",
            },
            crate::panel_extensions::ExtensionInfo {
                id: BUILT_IN_IDS[0].into(),
                name: "engine".into(),
                enabled: true,
                icon_data_url: None,
                origin: "unknown",
            },
            crate::panel_extensions::ExtensionInfo {
                id: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
                name: "Microsoft Edge PDF Viewer".into(),
                enabled: true,
                icon_data_url: None,
                origin: "unknown",
            },
        ];
        let items = enrich_with_paths(
            entries,
            &BTreeMap::from([(extension_id.to_owned(), root.path().to_path_buf())]),
            None,
        );
        assert_eq!(items[0].origin, "user");
        assert_eq!(
            items[0].icon_data_url.as_deref(),
            Some(
                format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&medium)
                )
                .as_str()
            )
        );
        assert_eq!(items[1].origin, "builtin");
        assert_eq!(items[2].origin, "unknown");
        let installing = crate::panel_extensions::ExtensionInfo {
            id: extension_id.into(),
            name: "fixture".into(),
            enabled: true,
            icon_data_url: None,
            origin: "unknown",
        };
        let stale = BTreeMap::from([(
            extension_id.to_owned(),
            root.path().join("missing-old-folder"),
        )]);
        let installed = enrich_with_paths(vec![installing], &stale, Some(root.path()));
        assert!(installed[0].icon_data_url.is_some());
        let pdf = crate::panel_extensions::ExtensionInfo {
            id: BUILT_IN_IDS[1].into(),
            name: "pdf".into(),
            enabled: true,
            icon_data_url: None,
            origin: "unknown",
        };
        assert_eq!(
            enrich_with_paths(vec![pdf], &BTreeMap::new(), None)[0].origin,
            "builtin"
        );
        assert_eq!(icon_data(root.path()), items[0].icon_data_url);
        std::fs::write(
            root.path().join("manifest.json"),
            r#"{"icons":{"48":"./medium.png"}}"#,
        )
        .unwrap();
        assert_eq!(icon_data(root.path()), items[0].icon_data_url);
        std::fs::write(
            root.path().join("medium.png"),
            b"\x89PNG\r\n\x1a\nchanged-longer",
        )
        .unwrap();
        assert_ne!(icon_data(root.path()), items[0].icon_data_url);
    }

    #[test]
    fn icon_manifest_cannot_read_outside_or_return_active_content() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("extension");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(root.path().join("outside.png"), b"\x89PNG\r\n\x1a\nsecret").unwrap();
        std::fs::write(
            folder.join("manifest.json"),
            r#"{"icons":{"48":"../outside.png"}}"#,
        )
        .unwrap();
        assert!(icon_data(&folder).is_none());
        std::fs::write(folder.join("evil.svg"), b"<svg onload='alert(1)'></svg>").unwrap();
        std::fs::write(
            folder.join("manifest.json"),
            r#"{"icons":{"48":"evil.svg"}}"#,
        )
        .unwrap();
        assert!(icon_data(&folder).is_none());
        std::fs::write(folder.join("large.png"), vec![0; 256 * 1024 + 1]).unwrap();
        std::fs::write(
            folder.join("manifest.json"),
            r#"{"icons":{"48":"large.png"}}"#,
        )
        .unwrap();
        assert!(icon_data(&folder).is_none());
    }

    #[test]
    fn imported_candidates_and_copy_use_only_the_selected_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("chrome");
        let id = "abcdefghijklmnopabcdefghijklmnop";
        let extension = root
            .join("Default")
            .join("Extensions")
            .join(id)
            .join("1.2.3_0");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("manifest.json"),
            r#"{"name":"시험 확장","manifest_version":3,"version":"1.2.3"}"#,
        )
        .unwrap();
        std::fs::write(extension.join("sample.js"), "fixture").unwrap();
        let candidates = scan("chrome", &root);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "시험 확장");
        let source = candidate_path(&root, "Default", id, "1.2.3_0").unwrap();
        let destination = temp.path().join("copy");
        std::fs::create_dir(&destination).unwrap();
        copy_extension(&source, &destination).unwrap();
        assert_eq!(
            std::fs::read(destination.join("sample.js")).unwrap(),
            b"fixture"
        );
        assert!(candidate_path(&root, "..", id, "1.2.3_0").is_err());
        assert!(candidate_path(&root, "Default", "invalid", "1.2.3_0").is_err());
        assert!(extension.join("sample.js").exists());

        std::fs::create_dir_all(extension.join("_locales/ko")).unwrap();
        std::fs::write(
            extension.join("_locales/ko/messages.json"),
            r#"{"title":{"message":"한글 이름"}}"#,
        )
        .unwrap();
        std::fs::write(extension.join("manifest.json"), r#"{"name":"__MSG_title__","default_locale":"en","manifest_version":3,"version":"1.2.3"}"#).unwrap();
        assert_eq!(scan("chrome", &root)[0].name, "한글 이름");
    }

    #[test]
    fn pin_catalog_round_trips_without_losing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("catalog.json");
        let id = "abcdefghijklmnopabcdefghijklmnop";
        let mut catalog = Catalog::default();
        catalog.pinned.push(id.into());
        catalog.paths.insert(id.into(), temp.path().join("확장"));
        save(&file, &catalog).unwrap();
        let restored = load(&file).unwrap();
        assert_eq!(restored.pinned, vec![id]);
        assert_eq!(restored.paths[id], temp.path().join("확장"));
        std::fs::write(&file, b"bad").unwrap();
        assert!(matches!(load(&file), Err("catalog_invalid")));
    }

    #[test]
    fn popup_address_stays_in_the_same_extension() {
        let id = "abcdefghijklmnopabcdefghijklmnop";
        assert_eq!(
            popup_url(id, "popup.html?tab=1").unwrap(),
            format!("chrome-extension://{id}/popup.html?tab=1")
        );
        assert!(popup_url(id, "https://example.test/").is_err());
        assert!(popup_url(id, "//bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/popup.html").is_err());
    }
}
