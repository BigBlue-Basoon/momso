//! Download and verify Chrome Web Store CRX3 packages before unpacking.
use base64::Engine;
use std::{
    io::{Seek, SeekFrom, Write},
    path::Path,
};
use tauri::Emitter;

// Chromium's pinned Chrome Web Store publisher SPKI SHA-256, not an extension allowlist.
const STORE_KEY: crx3::KeyHash = crx3::KeyHash::from_bytes([
    0x61, 0xf7, 0xf2, 0xa6, 0xbf, 0xcf, 0x74, 0xcd, 0x0b, 0xc1, 0xfe, 0x24, 0x97, 0xcc, 0x9b, 0x04,
    0x25, 0x4c, 0x65, 0x8f, 0x79, 0xf2, 0x14, 0x53, 0x92, 0x86, 0x7e, 0xa8, 0x36, 0x63, 0x67, 0xcf,
]);

fn extension_id(input: &str) -> Result<String, &'static str> {
    let input = input.trim();
    let candidate = if input.len() == 32 && input.bytes().all(|b| (b'a'..=b'p').contains(&b)) {
        input.to_owned()
    } else {
        let url = reqwest::Url::parse(input).map_err(|_| "invalid_store_address")?;
        if url.scheme() != "https"
            || !matches!(
                url.host_str(),
                Some("chromewebstore.google.com" | "chrome.google.com")
            )
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err("invalid_store_address");
        }
        url.path_segments()
            .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
            .unwrap_or("")
            .to_owned()
    };
    if candidate.len() != 32 || !candidate.bytes().all(|b| (b'a'..=b'p').contains(&b)) {
        return Err("invalid_store_address");
    }
    Ok(candidate)
}

#[cfg(windows)]
fn webview_version() -> Result<String, &'static str> {
    use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
    let mut version = windows_core::PWSTR::null();
    unsafe {
        GetAvailableCoreWebView2BrowserVersionString(windows_core::PCWSTR::null(), &mut version)
    }
    .map_err(|_| "unavailable")?;
    let version = webview2_com::take_pwstr(version);
    let version = version.split_whitespace().next().ok_or("unavailable")?;
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return Err("unavailable");
    }
    Ok(version.to_owned())
}

fn unpack_verified(
    file: &mut std::fs::File,
    expected: &str,
    destination: &Path,
    publisher: &crx3::KeyHash,
) -> Result<(), &'static str> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "store_package_invalid")?;
    let pinned = [*publisher];
    let verified = crx3::verify_reader(&mut *file, &crx3::Policy::new().any_of(&pinned))
        .map_err(|_| "store_signature_invalid")?;
    if verified.id != expected || verified.diff {
        return Err("store_id_mismatch");
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "store_package_invalid")?;
    let mut archive = zip::ZipArchive::new(&mut *file).map_err(|_| "store_package_invalid")?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|_| "store_package_invalid")?;
        let path = entry.enclosed_name().ok_or("store_package_invalid")?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("store_package_invalid");
        }
        let target = destination.join(path);
        if entry.is_dir() {
            std::fs::create_dir_all(target).map_err(|_| "unavailable")?;
        } else {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|_| "unavailable")?;
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)
                .map_err(|_| "store_package_invalid")?;
            std::io::copy(&mut entry, &mut output).map_err(|_| "unavailable")?;
        }
    }
    let manifest_path = destination.join("manifest.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&manifest_path).map_err(|_| "store_package_invalid")?,
    )
    .map_err(|_| "store_package_invalid")?;
    let object = manifest.as_object_mut().ok_or("store_package_invalid")?;
    let key = base64::engine::general_purpose::STANDARD.encode(&verified.public_key);
    if object
        .get("key")
        .is_some_and(|existing| existing.as_str() != Some(key.as_str()))
    {
        return Err("store_id_mismatch");
    }
    object.insert("key".into(), key.into());
    std::fs::write(
        manifest_path,
        serde_json::to_vec(&manifest).map_err(|_| "store_package_invalid")?,
    )
    .map_err(|_| "unavailable")?;
    Ok(())
}

async fn download_package(
    client: &reqwest::Client,
    url: reqwest::Url,
    output: &mut std::fs::File,
) -> Result<(), &'static str> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "store_unavailable")?
        .error_for_status()
        .map_err(|_| "store_unavailable")?;
    while let Some(chunk) = response.chunk().await.map_err(|_| "store_unavailable")? {
        output.write_all(&chunk).map_err(|_| "unavailable")?;
    }
    Ok(())
}

#[cfg(all(test, windows))]
pub(crate) async fn install_signed_fixture(
    app: &tauri::AppHandle,
    root: &Path,
) -> Result<(), &'static str> {
    use crx3::{
        aws_lc::{EcdsaSigner, RsaSigner},
        Builder, Signer,
    };
    use std::io::Read;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
        .map_err(|_| "fixture_io")?;
    zip.write_all(br#"{"manifest_version":3,"name":"store fixture","version":"1.0"}"#)
        .map_err(|_| "fixture_io")?;
    let archive = zip.finish().map_err(|_| "fixture_io")?.into_inner();
    let developer = RsaSigner::generate().map_err(|_| "fixture_io")?;
    let store = EcdsaSigner::generate().map_err(|_| "fixture_io")?;
    let package = Builder::new()
        .signer(&developer)
        .signer(&store)
        .build(&archive)
        .map_err(|_| "fixture_io")?;
    let publisher = crx3::key_hash(store.public_key());
    let id = crx3::key_hash(developer.public_key()).id();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|_| "fixture_io")?;
    let address = listener.local_addr().map_err(|_| "fixture_io")?;
    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                package.len()
            );
            let _ = stream.write_all(&package);
        }
    });
    let mut file = tempfile::tempfile_in(root).map_err(|_| "fixture_io")?;
    download_package(
        &reqwest::Client::new(),
        format!("http://{address}/test.crx")
            .parse()
            .map_err(|_| "fixture_io")?,
        &mut file,
    )
    .await
    .map_err(|_| "fixture_download")?;
    server.join().map_err(|_| "fixture_io")?;
    let folder = root.join("store-fixture");
    std::fs::create_dir_all(&folder).map_err(|_| "fixture_io")?;
    unpack_verified(&mut file, &id, &folder, &publisher).map_err(|_| "fixture_unpack")?;
    let installed =
        crate::panel_extensions::perform(app, crate::panel_extensions::Operation::Install(folder))
            .await
            .map_err(|_| "fixture_install")?;
    if installed.first().is_none_or(|item| item.id != id) {
        return Err("fixture_id_mismatch");
    }
    crate::panel_extensions::perform(app, crate::panel_extensions::Operation::Remove(id))
        .await
        .map_err(|_| "fixture_remove")?;
    Ok(())
}

#[tauri::command]
pub(crate) async fn panel_extensions_install_store(
    app: tauri::AppHandle,
    address_or_id: String,
) -> Result<Vec<crate::panel_extensions::ExtensionInfo>, &'static str> {
    #[cfg(windows)]
    {
        let id = extension_id(&address_or_id)?;
        let mut url = reqwest::Url::parse("https://clients2.google.com/service/update2/crx")
            .map_err(|_| "unavailable")?;
        url.query_pairs_mut()
            .append_pair("response", "redirect")
            .append_pair("prodversion", &webview_version()?)
            .append_pair("acceptformat", "crx3")
            .append_pair("x", &format!("id={id}&installsource=ondemand&uc"));
        let client = reqwest::Client::builder()
            .https_only(true)
            .build()
            .map_err(|_| "unavailable")?;
        let parent = crate::panel_extension_catalog::extension_data_root(&app)?
            .join("panel-extension-files");
        std::fs::create_dir_all(&parent).map_err(|_| "unavailable")?;
        let mut package = tempfile::tempfile_in(&parent).map_err(|_| "unavailable")?;
        download_package(&client, url, &mut package).await?;
        let folder = tempfile::Builder::new()
            .prefix("store-")
            .tempdir_in(&parent)
            .map_err(|_| "unavailable")?;
        unpack_verified(&mut package, &id, folder.path(), &STORE_KEY)?;
        let path = folder.keep();
        let installed = crate::panel_extensions::perform(
            &app,
            crate::panel_extensions::Operation::Install(path.clone()),
        )
        .await?;
        let result = installed.first().ok_or("install_failed")?;
        if result.id != id {
            let _ = app.emit("panel-extensions-changed", ());
            return Err("store_id_mismatch");
        }
        if crate::panel_extension_catalog::remember_path(&app, &result.id, &path).is_err() {
            let _ = app.emit("panel-extensions-changed", ());
            return Err("installed_catalog_unavailable");
        }
        let _ = app.emit("panel-extensions-changed", ());
        Ok(installed)
    }
    #[cfg(not(windows))]
    {
        let _ = (app, address_or_id);
        Err("unsupported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crx3::{
        aws_lc::{EcdsaSigner, RsaSigner},
        Builder, Signer,
    };
    #[test]
    fn store_address_accepts_id_or_official_url() {
        let id = "abcdefghijklmnopabcdefghijklmnop";
        assert_eq!(extension_id(id).unwrap(), id);
        assert_eq!(
            extension_id(&format!(
                "https://chromewebstore.google.com/detail/test/{id}?hl=ko"
            ))
            .unwrap(),
            id
        );
        assert!(extension_id(&format!("https://example.test/{id}")).is_err());
    }
    #[test]
    fn signed_store_package_is_verified_before_extracting() {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("manifest.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"{"manifest_version":3,"name":"fixture","version":"1.0"}"#)
            .unwrap();
        let archive = zip.finish().unwrap().into_inner();
        let developer = RsaSigner::generate().unwrap();
        let store = EcdsaSigner::generate().unwrap();
        let package = Builder::new()
            .signer(&developer)
            .signer(&store)
            .build(&archive)
            .unwrap();
        let trusted = crx3::key_hash(store.public_key());
        let verified = crx3::verify(&package, &crx3::Policy::new().any_of(&[trusted])).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&package).unwrap();
        unpack_verified(&mut file, &verified.id, temp.path(), &trusted).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(temp.path().join("manifest.json")).unwrap())
                .unwrap();
        assert!(manifest["key"].as_str().is_some());
        assert_eq!(
            unpack_verified(
                &mut file,
                "abcdefghijklmnopabcdefghijklmnop",
                temp.path(),
                &trusted
            ),
            Err("store_id_mismatch")
        );
        assert_eq!(
            unpack_verified(&mut file, &verified.id, temp.path(), &STORE_KEY),
            Err("store_signature_invalid")
        );
        let mut changed = package.clone();
        *changed.last_mut().unwrap() ^= 1;
        let mut changed_file = tempfile::tempfile().unwrap();
        changed_file.write_all(&changed).unwrap();
        assert_eq!(
            unpack_verified(&mut changed_file, &verified.id, temp.path(), &trusted),
            Err("store_signature_invalid")
        );
    }
}
