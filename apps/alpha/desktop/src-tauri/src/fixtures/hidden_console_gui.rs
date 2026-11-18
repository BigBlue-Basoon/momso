#![windows_subsystem = "windows"]

#[path = "../hidden_process.rs"]
mod hidden_process;

use std::collections::HashSet;
use std::ffi::c_void;
use std::time::{Duration, Instant};

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(callback: extern "system" fn(*mut c_void, isize) -> i32, data: isize) -> i32;
    fn GetClassNameW(window: *mut c_void, name: *mut u16, count: i32) -> i32;
    fn IsWindowVisible(window: *mut c_void) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetConsoleWindow() -> *mut c_void;
}

extern "system" fn collect(window: *mut c_void, data: isize) -> i32 {
    let mut name = [0_u16; 128];
    let length = unsafe { GetClassNameW(window, name.as_mut_ptr(), name.len() as i32) };
    if length > 0
        && unsafe { IsWindowVisible(window) } != 0
        && String::from_utf16_lossy(&name[..length as usize]) == "ConsoleWindowClass"
    {
        let windows = unsafe { &mut *(data as *mut HashSet<usize>) };
        windows.insert(window as usize);
    }
    1
}

fn console_windows() -> HashSet<usize> {
    let mut windows = HashSet::new();
    unsafe { EnumWindows(collect, &mut windows as *mut _ as isize) };
    windows
}

fn main() {
    let output = std::env::args_os().nth(1).unwrap();
    let result = (|| -> Result<(), &'static str> {
        if !unsafe { GetConsoleWindow() }.is_null() {
            return Err("parent_has_console");
        }
        let before = console_windows();
        let mut child = hidden_process::hidden_command("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 3",
            ])
            .spawn()
            .map_err(|_| "spawn_failed")?;
        let until = Instant::now() + Duration::from_secs(2);
        let mut new_console = false;
        while Instant::now() < until {
            new_console |= console_windows().difference(&before).next().is_some();
            std::thread::sleep(Duration::from_millis(20));
        }
        if !child.wait().map_err(|_| "wait_failed")?.success() {
            return Err("child_failed");
        }
        if new_console {
            return Err("new_console_window");
        }
        Ok(())
    })();
    std::fs::write(output, result.err().unwrap_or("ok")).unwrap();
}
