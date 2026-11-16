//! Private Windows pipe for a future authenticated setup broker.
//! No setup tools are exposed until the broker and its callers are connected.
#![allow(dead_code)]
use std::{
    collections::HashMap,
    ffi::OsString,
    io, mem,
    os::windows::{ffi::OsStringExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree, HANDLE, INVALID_HANDLE_VALUE},
    Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    },
    Security::{GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        },
        Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId},
        Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct LocalAllocation(*mut core::ffi::c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn current_sid() -> io::Result<String> {
    unsafe {
        let mut token: HANDLE = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = OwnedHandle(token);
        let mut bytes = 0u32;
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut bytes);
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (bytes as usize).div_ceil(mem::size_of::<usize>())];
        if GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut bytes,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let user = &*(buffer.as_ptr() as *const TOKEN_USER);
        let mut text = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = LocalAllocation(text.cast());
        let mut len = 0usize;
        while *text.add(len) != 0 {
            len += 1;
        }
        let sid = OsString::from_wide(std::slice::from_raw_parts(text, len))
            .to_string_lossy()
            .into_owned();
        drop(allocation);
        Ok(sid)
    }
}

pub(crate) fn create() -> io::Result<(String, NamedPipeServer)> {
    let sid = current_sid()?;
    let descriptor: Vec<u16> = format!("D:P(A;;GA;;;{sid})\0").encode_utf16().collect();
    let mut raw = ptr::null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),
            1,
            &mut raw,
            ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let allocated = LocalAllocation(raw.cast());
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: raw,
            bInheritHandle: 0,
        };
        let name = format!(r"\\.\pipe\momso-setup-{}", uuid::Uuid::new_v4());
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(
                &name,
                (&mut attrs as *mut SECURITY_ATTRIBUTES).cast(),
            )?;
        drop(allocated);
        Ok((name, server))
    }
}

fn image_path(pid: u32) -> io::Result<std::path::PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        let handle = OwnedHandle(h);
        let mut buffer = vec![0u16; 32768];
        let mut len = buffer.len() as u32;
        if QueryFullProcessImageNameW(handle.0, 0, buffer.as_mut_ptr(), &mut len) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OsString::from_wide(&buffer[..len as usize]).into())
    }
}

fn descendant(pid: u32, ancestor: u32) -> bool {
    if ancestor == 0 {
        return false;
    }
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let snapshot = OwnedHandle(snapshot);
        let mut info: PROCESSENTRY32W = mem::zeroed();
        info.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut parents = HashMap::new();
        let mut ok = Process32FirstW(snapshot.0, &mut info);
        while ok != 0 {
            parents.insert(info.th32ProcessID, info.th32ParentProcessID);
            ok = Process32NextW(snapshot.0, &mut info);
        }
        let mut current = pid;
        for _ in 0..32 {
            if current == ancestor {
                return true;
            }
            let Some(parent) = parents.get(&current).copied() else {
                return false;
            };
            if parent == 0 || parent == current {
                return false;
            }
            current = parent;
        }
        false
    }
}

pub(crate) async fn accept(
    server: NamedPipeServer,
    provider_pid: u32,
    executable: &Path,
) -> io::Result<NamedPipeServer> {
    server.connect().await?;
    let mut pid = 0u32;
    let ok = unsafe { GetNamedPipeClientProcessId(server.as_raw_handle().cast(), &mut pid) };
    let valid_image = match (
        image_path(pid).and_then(std::fs::canonicalize),
        std::fs::canonicalize(executable),
    ) {
        (Ok(actual), Ok(expected)) => actual == expected,
        _ => false,
    };
    if ok == 0 || pid == 0 || !valid_image || !descendant(pid, provider_pid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "untrusted pipe client",
        ));
    }
    Ok(server)
}

pub(crate) fn trusted_server(client: &std::fs::File) -> bool {
    let mut pid = 0u32;
    let ok = unsafe { GetNamedPipeServerProcessId(client.as_raw_handle().cast(), &mut pid) };
    if ok == 0 || pid == 0 {
        return false;
    }
    let same_executable = match (
        image_path(pid).and_then(std::fs::canonicalize),
        std::env::current_exe().and_then(std::fs::canonicalize),
    ) {
        (Ok(actual), Ok(ours)) => actual == ours,
        _ => false,
    };
    same_executable && descendant(std::process::id(), pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::windows::named_pipe::ClientOptions;
    #[tokio::test]
    async fn accepts_only_expected_process_and_executable() {
        let (name, server) = create().unwrap();
        let _client = ClientOptions::new().open(&name).unwrap();
        let own = std::env::current_exe().unwrap();
        assert!(accept(server, std::process::id(), &own).await.is_ok());
        let (name, server) = create().unwrap();
        let _client = ClientOptions::new().open(&name).unwrap();
        assert!(
            matches!(accept(server, 0, &own).await, Err(e) if e.kind() == io::ErrorKind::PermissionDenied)
        );
    }
    #[tokio::test]
    async fn client_verifies_the_pipe_server_process() {
        let (name, server) = create().unwrap();
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&name)
            .unwrap();
        assert!(trusted_server(&client));
        assert!(accept(
            server,
            std::process::id(),
            &std::env::current_exe().unwrap()
        )
        .await
        .is_ok());
    }
    #[tokio::test]
    async fn child_client_helper() {
        let Ok(name) = std::env::var("MOMSO_SETUP_PIPE_TEST_NAME") else {
            return;
        };
        let _client = ClientOptions::new().open(name).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    #[tokio::test]
    async fn unrelated_client_parent_helper() {
        if std::env::var_os("MOMSO_SETUP_PIPE_TEST_WAIT").is_some() {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    #[tokio::test]
    async fn rejects_other_same_user_process_and_accepts_bound_child() {
        let own = std::env::current_exe().unwrap();
        for allowed in [true, false] {
            let (name, server) = create().unwrap();
            let mut child = std::process::Command::new(&own)
                .args(["--exact", "setup_pipe::tests::child_client_helper"])
                .env("MOMSO_SETUP_PIPE_TEST_NAME", &name)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let mut sibling = if allowed {
                None
            } else {
                Some(
                    std::process::Command::new(&own)
                        .args([
                            "--exact",
                            "setup_pipe::tests::unrelated_client_parent_helper",
                        ])
                        .env("MOMSO_SETUP_PIPE_TEST_WAIT", "1")
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .unwrap(),
                )
            };
            let expected = sibling.as_ref().map_or(std::process::id(), |p| p.id());
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                accept(server, expected, &own),
            )
            .await
            .unwrap();
            assert_eq!(result.is_ok(), allowed);
            drop(result);
            assert!(child.wait().unwrap().success());
            if let Some(ref mut sibling) = sibling {
                assert!(sibling.wait().unwrap().success());
            }
        }
    }
}
