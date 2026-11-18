//! Windows Chromium debugging pipe. Only the two intended handles are inherited.
#[cfg(windows)]
mod platform {
    use std::{
        ffi::{OsStr, OsString},
        fs::File,
        io::{Read, Write},
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        path::Path,
        ptr,
        sync::mpsc,
        thread,
        time::Duration,
    };
    use windows_sys::Win32::{
        Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0},
        Security::SECURITY_ATTRIBUTES,
        System::{
            Pipes::CreatePipe,
            Threading::{
                CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
                TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW,
                CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTUPINFOEXW,
            },
        },
    };

    fn pipe() -> std::io::Result<(OwnedHandle, OwnedHandle)> {
        let mut read: HANDLE = ptr::null_mut();
        let mut write: HANDLE = ptr::null_mut();
        let security = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe {
            (
                OwnedHandle::from_raw_handle(read),
                OwnedHandle::from_raw_handle(write),
            )
        })
    }

    fn no_inherit(handle: &OwnedHandle) -> std::io::Result<()> {
        if unsafe { SetHandleInformation(handle.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn quoted(value: &OsStr, output: &mut Vec<u16>) {
        output.push(b'"' as u16);
        let mut slashes = 0;
        for unit in value.encode_wide() {
            if unit == b'\\' as u16 {
                slashes += 1;
                continue;
            }
            if unit == b'"' as u16 {
                output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
            } else {
                output.extend(std::iter::repeat_n(b'\\' as u16, slashes));
            }
            slashes = 0;
            output.push(unit);
        }
        output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
        output.push(b'"' as u16);
    }

    fn environment() -> Vec<u16> {
        const SECRET_NAMES: &[&str] = &[
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "MOMSO_CODEX_API_KEY",
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
        ];
        let mut vars: Vec<_> = std::env::vars_os()
            .filter(|(name, _)| {
                !SECRET_NAMES
                    .iter()
                    .any(|secret| name.to_string_lossy().eq_ignore_ascii_case(secret))
            })
            .collect();
        vars.sort_by_key(|(name, _)| name.to_string_lossy().to_uppercase());
        let mut block = Vec::new();
        for (name, value) in vars {
            block.extend(name.encode_wide());
            block.push(b'=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        if block.len() == 1 {
            block.push(0);
        }
        block
    }

    struct Attributes {
        storage: Vec<usize>,
        initialized: bool,
    }
    impl Drop for Attributes {
        fn drop(&mut self) {
            if self.initialized {
                unsafe {
                    DeleteProcThreadAttributeList(self.storage.as_mut_ptr().cast());
                }
            }
        }
    }

    pub(crate) struct BrowserProcess(OwnedHandle);
    impl BrowserProcess {
        pub(crate) fn exited(&self) -> bool {
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) == WAIT_OBJECT_0 }
        }
        pub(crate) fn kill(&self) {
            unsafe {
                TerminateProcess(self.0.as_raw_handle(), 1);
                WaitForSingleObject(self.0.as_raw_handle(), 3000);
            }
        }
    }
    pub(crate) struct Pipe {
        writer: mpsc::SyncSender<Vec<u8>>,
        receiver: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    }
    impl Pipe {
        pub(crate) fn send(&mut self, value: &serde_json::Value) -> std::io::Result<()> {
            let mut data = serde_json::to_vec(value)?;
            if data.len() > 16 * 1024 * 1024 {
                return Err(std::io::Error::other("debug message too large"));
            }
            data.push(0);
            self.writer
                .try_send(data)
                .map_err(|_| std::io::Error::other("debug pipe unavailable"))
        }
        pub(crate) fn recv(&self) -> Result<std::io::Result<Vec<u8>>, mpsc::RecvTimeoutError> {
            self.receiver.recv_timeout(Duration::from_millis(30))
        }
    }
    fn read_frames(mut reader: impl Read, tx: mpsc::SyncSender<std::io::Result<Vec<u8>>>) {
        let mut buffered = Vec::new();
        let mut bytes = [0u8; 8192];
        loop {
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => {
                    for byte in &bytes[..count] {
                        if *byte == 0 {
                            if tx.send(Ok(std::mem::take(&mut buffered))).is_err() {
                                return;
                            }
                        } else {
                            buffered.push(*byte);
                            if buffered.len() > 16 * 1024 * 1024 {
                                let _ =
                                    tx.send(Err(std::io::Error::other("debug message too large")));
                                return;
                            }
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        }
    }
    pub(crate) fn spawn(
        executable: &Path,
        profile: &Path,
        visible: bool,
    ) -> std::io::Result<(BrowserProcess, Pipe)> {
        let (child_read, parent_write) = pipe()?;
        let (parent_read, child_write) = pipe()?;
        no_inherit(&parent_write)?;
        no_inherit(&parent_read)?;
        let handles = [child_read.as_raw_handle(), child_write.as_raw_handle()];
        let mut size = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        }
        if size == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut attributes = Attributes {
            storage: vec![0usize; size.div_ceil(std::mem::size_of::<usize>())],
            initialized: false,
        };
        let list = attributes.storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        attributes.initialized = true;
        if unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(&handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut user_data = OsString::from("--user-data-dir=");
        user_data.push(profile.as_os_str());
        let io_arg = format!(
            "--remote-debugging-io-pipes={},{}",
            handles[0] as usize, handles[1] as usize
        );
        let mut args = vec![
            executable.as_os_str().to_owned(),
            user_data,
            "--remote-debugging-pipe".into(),
            io_arg.into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-background-networking".into(),
            "--disable-extensions".into(),
            "--no-startup-window".into(),
        ];
        if !visible {
            args.push("--headless=new".into());
        }
        let mut line = Vec::new();
        for (index, arg) in args.iter().enumerate() {
            if index > 0 {
                line.push(b' ' as u16);
            }
            quoted(arg, &mut line);
        }
        line.push(0);
        let mut app: Vec<u16> = executable.as_os_str().encode_wide().collect();
        app.push(0);
        let env = environment();
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.lpAttributeList = list;
        let mut info = PROCESS_INFORMATION::default();
        let created = unsafe {
            CreateProcessW(
                app.as_ptr(),
                line.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                env.as_ptr().cast(),
                ptr::null(),
                &startup.StartupInfo,
                &mut info,
            )
        };
        if created == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
        drop(child_read);
        drop(child_write);
        let (tx, receiver) = mpsc::sync_channel(256);
        thread::spawn(move || read_frames(File::from(parent_read), tx));
        let (writer, pending) = mpsc::sync_channel::<Vec<u8>>(64);
        thread::spawn(move || {
            let mut pipe = File::from(parent_write);
            while let Ok(message) = pending.recv() {
                if pipe.write_all(&message).is_err() {
                    break;
                }
            }
        });
        Ok((BrowserProcess(process), Pipe { writer, receiver }))
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn nul_framing_preserves_multiple_messages_and_unicode_arguments() {
            let (tx, rx) = mpsc::sync_channel(4);
            read_frames(std::io::Cursor::new(b"{\"id\":1}\0{\"id\":2}\0"), tx);
            assert_eq!(rx.recv().unwrap().unwrap(), b"{\"id\":1}");
            assert_eq!(rx.recv().unwrap().unwrap(), b"{\"id\":2}");
            let mut value = Vec::new();
            quoted(OsStr::new("C:\\사용자 이름\\끝\\"), &mut value);
            assert_eq!(
                String::from_utf16(&value).unwrap(),
                "\"C:\\사용자 이름\\끝\\\\\""
            );
        }
        #[test]
        fn blocked_writer_does_not_block_the_command_thread() {
            let (writer, _pending) = mpsc::sync_channel(1);
            writer.try_send(vec![1]).unwrap();
            let (_tx, receiver) = mpsc::sync_channel(1);
            let mut pipe = Pipe { writer, receiver };
            let start = std::time::Instant::now();
            assert!(pipe.send(&serde_json::json!({"id":1})).is_err());
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }
}

#[cfg(windows)]
pub(crate) use platform::{spawn, BrowserProcess, Pipe};

#[cfg(not(windows))]
mod unsupported {
    pub(crate) struct BrowserProcess;
    impl BrowserProcess {
        pub(crate) fn exited(&self) -> bool {
            true
        }
        pub(crate) fn kill(&self) {}
    }
    pub(crate) struct Pipe;
    impl Pipe {
        pub(crate) fn send(&mut self, _: &serde_json::Value) -> std::io::Result<()> {
            Err(std::io::Error::other("unavailable"))
        }
        pub(crate) fn recv(
            &self,
        ) -> Result<std::io::Result<Vec<u8>>, std::sync::mpsc::RecvTimeoutError> {
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        }
    }
    pub(crate) fn spawn(
        _: &std::path::Path,
        _: &std::path::Path,
        _: bool,
    ) -> std::io::Result<(BrowserProcess, Pipe)> {
        Err(std::io::Error::other("unavailable"))
    }
}
#[cfg(not(windows))]
pub(crate) use unsupported::{spawn, BrowserProcess, Pipe};
