//! Host-only saved login storage. This crate does not implement an HTTP server.
pub mod backend;

#[cfg(target_os = "linux")]
pub mod linux;
