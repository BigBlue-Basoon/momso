use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceTypingStatus {
    state: &'static str,
    microphone: &'static str,
    language: &'static str,
    permission: &'static str,
    network: &'static str,
}

#[derive(Debug, Serialize)]
pub struct VoiceTypingOpen {
    state: &'static str,
}

#[tauri::command]
pub fn voice_typing_status() -> VoiceTypingStatus {
    VoiceTypingStatus {
        state: if cfg!(windows) {
            "unknown"
        } else {
            "unsupported"
        },
        microphone: microphone_status(),
        language: "unknown",
        permission: "unknown",
        network: "unknown",
    }
}

#[cfg(windows)]
fn microphone_status() -> &'static str {
    // This legacy device count is advisory, not a privacy or readiness check.
    if unsafe { windows_sys::Win32::Media::Audio::waveInGetNumDevs() } == 0 {
        "no_legacy_input_detected"
    } else {
        "legacy_input_detected"
    }
}

#[cfg(not(windows))]
fn microphone_status() -> &'static str {
    "unknown"
}

#[tauri::command]
pub fn voice_typing_open(window: tauri::WebviewWindow) -> VoiceTypingOpen {
    let focused = window.is_focused().unwrap_or(false);
    VoiceTypingOpen {
        state: open(focused),
    }
}

#[cfg(not(windows))]
fn open(_: bool) -> &'static str {
    "unsupported"
}

#[cfg(windows)]
fn open(focused: bool) -> &'static str {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

    if !focused {
        return "not_focused";
    }
    // Do not interfere with keys the user is physically holding.
    if [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, b'H' as u16]
        .iter()
        .any(|key| unsafe { GetAsyncKeyState(*key as i32) } < 0)
    {
        return "keys_busy";
    }
    let inputs = shortcut();
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        "requested"
    } else {
        // Release only keys whose down event was inserted but up event was not.
        let releases = match sent {
            1 => &inputs[3..4],
            2 => &inputs[2..4],
            3 => &inputs[3..4],
            _ => &inputs[0..0],
        };
        if !releases.is_empty() {
            unsafe {
                SendInput(
                    releases.len() as u32,
                    releases.as_ptr(),
                    std::mem::size_of::<INPUT>() as i32,
                )
            };
        }
        "input_rejected"
    }
}

#[cfg(windows)]
fn shortcut() -> [windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT; 4] {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_LWIN,
    };
    fn key(code: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: code,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        }
    }
    [
        key(VK_LWIN, 0),
        key(b'H' as u16, 0),
        key(b'H' as u16, KEYEVENTF_KEYUP),
        key(VK_LWIN, KEYEVENTF_KEYUP),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn unavailable_window_does_not_send_input() {
        #[cfg(windows)]
        assert_eq!(super::open(false), "not_focused");
        #[cfg(not(windows))]
        assert_eq!(super::open(false), "unsupported");
    }

    #[test]
    fn status_never_claims_windows_readiness() {
        let result = super::voice_typing_status();
        assert_eq!(result.language, "unknown");
        assert_eq!(result.permission, "unknown");
        assert_eq!(result.network, "unknown");
    }

    #[cfg(windows)]
    #[test]
    fn shortcut_presses_and_releases_win_h_in_order() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_LWIN};
        let events = super::shortcut();
        let pairs: Vec<_> = events
            .iter()
            .map(|event| unsafe { (event.Anonymous.ki.wVk, event.Anonymous.ki.dwFlags) })
            .collect();
        assert_eq!(
            pairs,
            vec![
                (VK_LWIN, 0),
                (b'H' as u16, 0),
                (b'H' as u16, KEYEVENTF_KEYUP),
                (VK_LWIN, KEYEVENTF_KEYUP)
            ]
        );
    }
}
