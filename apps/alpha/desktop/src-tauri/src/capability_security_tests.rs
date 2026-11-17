#[test]
fn opener_path_is_limited_to_the_data_directory_itself() {
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    let permission = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["identifier"] == "opener:allow-open-path")
        .unwrap();
    let allowed = permission["allow"].as_array().unwrap();
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0]["path"], "$APPCONFIG");
}

#[test]
fn webview_cannot_bypass_host_url_validation_using_opener() {
    let capability: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    assert!(!capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry.as_str() == Some("opener:default")
            || entry.as_str() == Some("opener:allow-open-url")
            || entry["identifier"] == "opener:allow-open-url"));
}

#[test]
fn identifier_path_boundaries_reject_windows_escape_forms() {
    for bad in [
        "../outside",
        "C:\\outside",
        "\\\\server\\share",
        "NUL",
        "con",
        "lpt1",
        "bad.",
        "bad ",
        "a:b",
        "a/b",
        "a\\b",
        "",
        &"x".repeat(400),
    ] {
        assert!(!crate::archive_zet::valid_zet_id(bad));
        assert!(!crate::archive_receipt::valid_id(bad));
        assert!(!crate::archive_receipt::valid_chat_id(bad));
        assert!(!crate::skills::valid_name(bad));
    }
    assert!(crate::skills::valid_name("yoga-lesson"));
}
