//! Headless browser adapter for a momso server. No HTTP server or UI is included.
#[cfg(all(feature = "fixture-loopback", not(debug_assertions)))]
compile_error!("fixture-loopback is only for isolated debug tests");
#[path = "../../desktop/src-tauri/src/browser_engine.rs"]
pub mod engine;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub mod tools;

#[cfg(target_os = "linux")]
pub mod vault;

#[cfg(target_os = "linux")]
pub use linux::{HeadlessBrowser, LoginRequest, LoginSecret, Permission, ServerBrowsers};

#[cfg(all(test, target_os = "linux"))]
mod s31_fixture;

#[cfg(all(test, target_os = "linux"))]
mod s33_fixture;
