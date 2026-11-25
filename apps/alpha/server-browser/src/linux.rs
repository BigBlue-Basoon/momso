use crate::engine::{BrowserEngine, DriverError, Element, PageState};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, ToSocketAddrs},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::OpenOptionsExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroize;

pub use crate::engine::BrowserPermission as Permission;

/// The caller obtains this from its own credential store, never from AI arguments.
pub struct LoginSecret {
    username: String,
    password: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LoginRequest {
    pub origin: String,
    /// Trusted host only; never returned to the AI as a login argument.
    pub top_url: String,
    pub reason: &'static str,
}
impl LoginSecret {
    pub fn new(username: String, password: String) -> Self {
        Self { username, password }
    }
}
impl Drop for LoginSecret {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, _, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || first == 0
                || (first == 100 && (64..=127).contains(&second)))
        }
        IpAddr::V6(ip) => {
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v| !public_ip(IpAddr::V4(v))))
        }
    }
}

fn allowed_url(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let host = match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            #[cfg(any(test, all(feature = "fixture-loopback", debug_assertions)))]
            if ip == std::net::Ipv4Addr::LOCALHOST {
                return true;
            }
            return public_ip(IpAddr::V4(ip));
        }
        Some(url::Host::Ipv6(ip)) => return public_ip(IpAddr::V6(ip)),
        Some(url::Host::Domain(host)) => host,
        None => return false,
    };
    let normalized = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
        || normalized.ends_with(".internal")
    {
        return false;
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let Ok(mut addresses) = (host, port).to_socket_addrs() else {
        return false;
    };
    let Some(first) = addresses.next() else {
        return false;
    };
    public_ip(first.ip()) && addresses.all(|address| public_ip(address.ip()))
}

fn allowed_request(value: &str) -> bool {
    if value.starts_with("data:") || value.starts_with("blob:") {
        return true;
    }
    let Ok(mut url) = url::Url::parse(value) else {
        return false;
    };
    let scheme = match url.scheme() {
        "ws" => Some("http"),
        "wss" => Some("https"),
        _ => None,
    };
    if scheme.is_some_and(|scheme| url.set_scheme(scheme).is_err()) {
        return false;
    }
    allowed_url(url.as_str())
}

fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn dup_high(fd: &OwnedFd) -> std::io::Result<OwnedFd> {
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 10) };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(result) })
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}

fn fd_path(file: &File) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

fn profile_guard(profile: &File) -> std::io::Result<File> {
    let guard = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(fd_path(profile).join(".momso-profile.lock"))?;
    if unsafe { libc::flock(guard.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(guard)
}

/// Delete only old Chromium caches in inactive, app-named profiles. Cookie and
/// local storage paths are deliberately not visited. A pinned directory fd and
/// no-follow opens prevent parent symlinks from redirecting deletion.
pub fn prune_inactive_caches(root: &Path, now: SystemTime) -> usize {
    let Ok(root) = open_directory(root) else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(fd_path(&root)) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.len() != 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let Ok(profile) = open_directory(&entry.path()) else {
            continue;
        };
        let Ok(_guard) = profile_guard(&profile) else {
            continue;
        };
        let inactive = _guard
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= Duration::from_secs(30 * 24 * 60 * 60));
        if !inactive {
            continue;
        }
        if std::fs::symlink_metadata(fd_path(&profile).join("SingletonLock")).is_ok() {
            continue;
        }
        let Ok(default) = open_directory(&fd_path(&profile).join("Default")) else {
            continue;
        };
        for name in ["Cache", "Code Cache", "GPUCache"] {
            let cache = fd_path(&default).join(name);
            let Ok(meta) = std::fs::symlink_metadata(&cache) else {
                continue;
            };
            if !meta.file_type().is_dir() {
                continue;
            }
            let old = meta
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= Duration::from_secs(30 * 24 * 60 * 60));
            if old && std::fs::remove_dir_all(cache).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

pub struct HeadlessBrowser {
    child: Child,
    _profile_guard: File,
    writer: Arc<Mutex<File>>,
    receiver: mpsc::Receiver<Vec<u8>>,
    sequence: u64,
    session: Arc<Mutex<String>>,
    // Decisions belong to this chat session and expire when it is closed.
    decisions: HashMap<String, bool>,
    action_origin: Option<String>,
    login_secrets: Vec<zeroize::Zeroizing<String>>,
    one_time_hashes: Vec<([u8; 32], usize)>,
}

impl HeadlessBrowser {
    pub fn launch(executable: &Path, profile: &Path) -> Result<Self, DriverError> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(profile).map_err(|_| DriverError::Unavailable)?;
        if std::fs::symlink_metadata(profile)
            .map_err(|_| DriverError::Unavailable)?
            .file_type()
            .is_symlink()
        {
            return Err(DriverError::Refused("invalid_profile"));
        }
        std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| DriverError::Unavailable)?;
        let directory = open_directory(profile).map_err(|_| DriverError::Unavailable)?;
        let guard = profile_guard(&directory).map_err(|_| DriverError::Unavailable)?;
        guard
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()))
            .map_err(|_| DriverError::Unavailable)?;
        let (child_read, parent_write) = pipe().map_err(|_| DriverError::Unavailable)?;
        let (parent_read, child_write) = pipe().map_err(|_| DriverError::Unavailable)?;
        let child_read_high = dup_high(&child_read).map_err(|_| DriverError::Unavailable)?;
        let child_write_high = dup_high(&child_write).map_err(|_| DriverError::Unavailable)?;
        drop(child_read);
        drop(child_write);
        let child_read = child_read_high;
        let child_write = child_write_high;
        let r = child_read.as_raw_fd();
        let w = child_write.as_raw_fd();
        let mut command = Command::new(executable);
        command
            .arg("--headless=new")
            .arg("--remote-debugging-pipe")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--disk-cache-size=104857600")
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-extensions",
                "--no-startup-window",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.env_clear();
        for key in ["PATH", "LANG", "LC_ALL", "TMPDIR", "TZ"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .env("HOME", profile)
            .env("XDG_CONFIG_HOME", profile.join(".config"))
            .env("XDG_CACHE_HOME", profile.join(".cache"));
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(r, 3) < 0 || libc::dup2(w, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(r);
                libc::close(w);
                // The browser is expendable; do not inherit the server's OOM preference.
                let fd = libc::open(
                    c"/proc/self/oom_score_adj".as_ptr(),
                    libc::O_WRONLY | libc::O_CLOEXEC,
                );
                if fd < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let result = libc::write(fd, b"500".as_ptr().cast(), 3);
                libc::close(fd);
                if result != 3 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|_| DriverError::Unavailable)?;
        drop(child_read);
        drop(child_write);
        let mut reader = File::from(parent_read);
        let writer = Arc::new(Mutex::new(File::from(parent_write)));
        let event_writer = Arc::clone(&writer);
        let session = Arc::new(Mutex::new(String::new()));
        let event_session = Arc::clone(&session);
        let (sender, receiver) = mpsc::sync_channel(512);
        thread::spawn(move || {
            let mut bytes = [0u8; 8192];
            let mut frame = Vec::new();
            let mut event_id = 1_000_000_000_u64;
            let mut waiting: HashMap<u64, (String, String, bool)> = HashMap::new();
            while let Ok(n) = reader.read(&mut bytes) {
                if n == 0 {
                    break;
                }
                for byte in &bytes[..n] {
                    if *byte == 0 {
                        let bytes = std::mem::take(&mut frame);
                        let Ok(message) = serde_json::from_slice::<Value>(&bytes) else {
                            return;
                        };
                        if message.get("method").and_then(Value::as_str)
                            == Some("Target.attachedToTarget")
                        {
                            let Some(sid) =
                                message.pointer("/params/sessionId").and_then(Value::as_str)
                            else {
                                return;
                            };
                            let Some(target) = message
                                .pointer("/params/targetInfo/targetId")
                                .and_then(Value::as_str)
                            else {
                                return;
                            };
                            let is_page = message
                                .pointer("/params/targetInfo/type")
                                .and_then(Value::as_str)
                                == Some("page");
                            event_id += 1;
                            let reply = json!({"id":event_id,"sessionId":sid,"method":"Fetch.enable","params":{"patterns":[{"urlPattern":"*","requestStage":"Request"}]}});
                            let Ok(mut output) = serde_json::to_vec(&reply) else {
                                return;
                            };
                            output.push(0);
                            if event_writer
                                .lock()
                                .ok()
                                .and_then(|mut w| w.write_all(&output).ok())
                                .is_none()
                            {
                                return;
                            }
                            waiting.insert(event_id, (sid.to_owned(), target.to_owned(), is_page));
                            continue;
                        }
                        if let Some(id) = message.get("id").and_then(Value::as_u64) {
                            if let Some((sid, target, is_page)) = waiting.remove(&id) {
                                event_id += 1;
                                let reply = if message.get("error").is_some() {
                                    json!({"id":event_id,"method":"Target.closeTarget","params":{"targetId":target}})
                                } else {
                                    if is_page {
                                        let Ok(mut active) = event_session.lock() else {
                                            return;
                                        };
                                        *active = sid.clone();
                                    }
                                    json!({"id":event_id,"sessionId":sid,"method":"Runtime.runIfWaitingForDebugger","params":{}})
                                };
                                let Ok(mut output) = serde_json::to_vec(&reply) else {
                                    return;
                                };
                                output.push(0);
                                if event_writer
                                    .lock()
                                    .ok()
                                    .and_then(|mut w| w.write_all(&output).ok())
                                    .is_none()
                                {
                                    return;
                                }
                                continue;
                            }
                        }
                        if message.get("method").and_then(Value::as_str)
                            == Some("Fetch.requestPaused")
                        {
                            let Some(request) =
                                message.pointer("/params/requestId").and_then(Value::as_str)
                            else {
                                return;
                            };
                            let url = message
                                .pointer("/params/request/url")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let (method, params) = if allowed_request(url) {
                                ("Fetch.continueRequest", json!({"requestId":request}))
                            } else {
                                (
                                    "Fetch.failRequest",
                                    json!({"requestId":request,"errorReason":"BlockedByClient"}),
                                )
                            };
                            event_id += 1;
                            let mut reply = json!({"id":event_id,"method":method,"params":params});
                            if let Some(session) = message.get("sessionId") {
                                reply["sessionId"] = session.clone();
                            }
                            let Ok(mut output) = serde_json::to_vec(&reply) else {
                                return;
                            };
                            output.push(0);
                            if event_writer
                                .lock()
                                .ok()
                                .and_then(|mut writer| writer.write_all(&output).ok())
                                .is_none()
                            {
                                return;
                            }
                            continue;
                        }
                        if message
                            .get("id")
                            .and_then(Value::as_u64)
                            .is_some_and(|id| id >= 1_000_000_000)
                        {
                            continue;
                        }
                        if message.get("id").is_none() {
                            continue;
                        }
                        if sender.send(bytes).is_err() {
                            return;
                        }
                    } else {
                        frame.push(*byte);
                        if frame.len() > 16 * 1024 * 1024 {
                            return;
                        }
                    }
                }
            }
        });
        let mut browser = Self {
            child,
            _profile_guard: guard,
            writer,
            receiver,
            sequence: 0,
            session,
            decisions: HashMap::new(),
            action_origin: None,
            login_secrets: Vec::new(),
            one_time_hashes: Vec::new(),
        };
        let target = browser.call_root("Target.createTarget", json!({"url":"about:blank"}))?;
        let id = target
            .get("targetId")
            .and_then(Value::as_str)
            .ok_or(DriverError::Unavailable)?;
        let attached = browser.call_root(
            "Target.attachToTarget",
            json!({"targetId":id,"flatten":true}),
        )?;
        *browser
            .session
            .lock()
            .map_err(|_| DriverError::Unavailable)? = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or(DriverError::Unavailable)?
            .to_owned();
        browser.call("Page.enable", json!({}))?;
        browser.call(
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
        )?;
        browser.call_root(
            "Target.setAutoAttach",
            json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
        )?;
        // Chromium can reset its inherited score during startup; set it again
        // after CDP has finished initializing the browser process.
        std::fs::write(
            format!("/proc/{}/oom_score_adj", browser.child.id()),
            b"500",
        )
        .map_err(|_| DriverError::Unavailable)?;
        Ok(browser)
    }

    fn send(&mut self, method: &str, params: Value, session: bool) -> Result<u64, DriverError> {
        self.sequence += 1;
        let mut request = json!({"id":self.sequence,"method":method,"params":params});
        if session {
            request["sessionId"] =
                json!(*self.session.lock().map_err(|_| DriverError::Unavailable)?);
        }
        let mut bytes = serde_json::to_vec(&request).map_err(|_| DriverError::Unavailable)?;
        bytes.push(0);
        self.writer
            .lock()
            .map_err(|_| DriverError::Unavailable)?
            .write_all(&bytes)
            .map_err(|_| DriverError::Unavailable)?;
        Ok(self.sequence)
    }

    fn response(&mut self, id: u64) -> Result<Value, DriverError> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or(DriverError::Unavailable)?;
            let bytes = self
                .receiver
                .recv_timeout(remaining)
                .map_err(|_| DriverError::Unavailable)?;
            let message: Value =
                serde_json::from_slice(&bytes).map_err(|_| DriverError::Unavailable)?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if message.get("error").is_some() {
                    return Err(DriverError::Unavailable);
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or(DriverError::Unavailable);
            }
        }
    }

    fn call_root(&mut self, method: &str, params: Value) -> Result<Value, DriverError> {
        let id = self.send(method, params, false)?;
        self.response(id)
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value, DriverError> {
        let id = self.send(method, params, true)?;
        self.response(id)
    }

    fn current_frame(&mut self) -> Result<(String, String, String), DriverError> {
        let tree = self.call("Page.getFrameTree", json!({}))?;
        let url = tree
            .pointer("/frameTree/frame/url")
            .and_then(Value::as_str)
            .ok_or(DriverError::Unavailable)?
            .to_owned();
        if !allowed_url(&url) {
            return Err(DriverError::Blocked(String::new()));
        }
        let frame = tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or(DriverError::Unavailable)?
            .to_owned();
        let loader = tree
            .pointer("/frameTree/frame/loaderId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        Ok((url, frame, loader))
    }

    fn current_url(&mut self) -> Result<String, DriverError> {
        Ok(self.current_frame()?.0)
    }

    pub fn human_login_request(
        &mut self,
        reason: &'static str,
    ) -> Result<LoginRequest, DriverError> {
        let url = self.current_url()?;
        let origin = url::Url::parse(&url)
            .map_err(|_| DriverError::Unavailable)?
            .origin()
            .ascii_serialization();
        Ok(LoginRequest {
            origin,
            top_url: url,
            reason,
        })
    }

    /// Host-only challenge detection; neither the code nor field values are returned.
    pub fn login_challenge(&mut self) -> Result<&'static str, DriverError> {
        let value = self.evaluate(r#"(()=>{if(document.querySelector('iframe[src*=recaptcha],iframe[src*=hcaptcha],.g-recaptcha,.h-captcha'))return 'captcha';if(document.querySelector('input[autocomplete~=one-time-code]'))return 'one_time_code';if(document.querySelector('[autocomplete~=webauthn]'))return 'passkey';if(document.querySelector('input[type=password]'))return 'login';return 'additional_action'})()"#, true)?;
        Ok(match value.as_str() {
            Some("captcha") => "captcha",
            Some("one_time_code") => "one_time_code",
            Some("passkey") => "passkey",
            Some("login") => "login",
            _ => "additional_action",
        })
    }

    /// Accept a human-supplied code only for the exact unchanged top-level document.
    pub fn submit_one_time_code(
        &mut self,
        expected_url: &str,
        expected_loader: &str,
        code: zeroize::Zeroizing<String>,
    ) -> Result<(), DriverError> {
        if code.is_empty()
            || code.len() > 32
            || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(DriverError::Refused("invalid_code"));
        }
        let (url, _, loader) = self.current_frame()?;
        if url != expected_url || loader != expected_loader {
            return Err(DriverError::TargetChanged);
        }
        if self.one_time_hashes.len() >= 32 {
            return Err(DriverError::Refused("verification_limit"));
        }
        self.one_time_hashes
            .push((Sha256::digest(code.as_bytes()).into(), code.len()));
        let encoded = zeroize::Zeroizing::new(
            serde_json::to_string(code.as_str()).map_err(|_| DriverError::Unavailable)?,
        );
        let script = zeroize::Zeroizing::new(format!(
            r#"(()=>{{if(document.querySelector('iframe,input[type=password],.g-recaptcha,.h-captcha'))return 'verification';const inputs=[...document.querySelectorAll('input[autocomplete~=one-time-code]')];if(inputs.length!==1||!inputs[0].form)return 'verification';const f=inputs[0].form;if(new URL(f.action,location.href).origin!==location.origin||f.method.toLowerCase()!=='post')return 'verification';const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;set.call(inputs[0],{value});inputs[0].dispatchEvent(new Event('input',{{bubbles:true}}));f.requestSubmit();return 'ok'}})()"#,
            value = *encoded
        ));
        match self
            .evaluate_in_document(&script, true, Some((expected_url, expected_loader)))?
            .as_str()
        {
            Some("ok") => Ok(()),
            _ => Err(DriverError::HumanRequired("verification")),
        }
    }

    pub fn one_time_code_document(&mut self) -> Result<(String, String), DriverError> {
        let (url, _, loader) = self.current_frame()?;
        Ok((url, loader))
    }

    fn evaluate(&mut self, expression: &str, allow_login: bool) -> Result<Value, DriverError> {
        self.evaluate_in_document(expression, allow_login, None)
    }

    fn evaluate_in_document(
        &mut self,
        expression: &str,
        allow_login: bool,
        expected: Option<(&str, &str)>,
    ) -> Result<Value, DriverError> {
        let tree = self.call("Page.getFrameTree", json!({}))?;
        fn collect(tree: &Value, frames: &mut Vec<String>) -> Result<(), DriverError> {
            if let Some(children) = tree.get("childFrames") {
                for child in children
                    .as_array()
                    .ok_or(DriverError::HumanRequired("verification"))?
                {
                    frames.push(
                        child
                            .pointer("/frame/id")
                            .and_then(Value::as_str)
                            .ok_or(DriverError::HumanRequired("verification"))?
                            .to_owned(),
                    );
                    collect(child, frames)?;
                }
            }
            Ok(())
        }
        let mut children = Vec::new();
        collect(
            tree.get("frameTree").ok_or(DriverError::Unavailable)?,
            &mut children,
        )?;
        for frame in children {
            let context = self
                .call(
                    "Page.createIsolatedWorld",
                    json!({"frameId":frame,"worldName":"momso-browser-driver"}),
                )
                .map_err(|_| DriverError::HumanRequired("verification"))?
                .get("executionContextId")
                .and_then(Value::as_i64)
                .ok_or(DriverError::HumanRequired("verification"))?;
            let result = self.call("Runtime.evaluate", json!({"expression":"(()=>{if(document.querySelector('input[type=password],input[autocomplete~=current-password],input[autocomplete~=new-password],input[autocomplete~=one-time-code]'))return 'login';if(document.querySelector('iframe[src*=recaptcha],iframe[src*=hcaptcha]'))return 'captcha';return 'ok'})()","contextId":context,"returnByValue":true})).map_err(|_| DriverError::HumanRequired("verification"))?;
            match result.pointer("/result/value").and_then(Value::as_str) {
                Some("ok") => {}
                Some("login") => return Err(DriverError::HumanRequired("login")),
                Some("captcha") => return Err(DriverError::HumanRequired("captcha")),
                _ => return Err(DriverError::HumanRequired("verification")),
            }
        }
        let (url, frame, loader) = self.current_frame()?;
        if expected.is_some_and(|(expected_url, expected_loader)| {
            url != expected_url || loader != expected_loader
        }) {
            return Err(DriverError::TargetChanged);
        }
        let context = self
            .call(
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":"momso-browser-driver"}),
            )?
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or(DriverError::Unavailable)?;
        if self.current_frame()?.2 != loader {
            return Err(DriverError::TargetChanged);
        }
        let url_json = serde_json::to_string(&url).map_err(|_| DriverError::Unavailable)?;
        let expression = if allow_login {
            format!("(()=>{{if(location.href!=={url_json})return {{changed:true}};return ({expression});}})()")
        } else {
            format!("(()=>{{if(location.href!=={url_json})return {{changed:true}};if(document.querySelector('input[type=password],input[autocomplete~=current-password],input[autocomplete~=new-password],input[autocomplete~=one-time-code]'))return {{human:'login'}};if(document.querySelector('iframe[src*=recaptcha],iframe[src*=hcaptcha]'))return {{human:'captcha'}};return ({expression});}})()")
        };
        let result = self.call("Runtime.evaluate", json!({"expression":expression,"contextId":context,"returnByValue":true,"userGesture":true}))?;
        if result.get("exceptionDetails").is_some() {
            return Err(DriverError::Unavailable);
        }
        let value = result
            .pointer("/result/value")
            .cloned()
            .ok_or(DriverError::Unavailable)?;
        if value.get("changed").and_then(Value::as_bool) == Some(true) {
            return Err(DriverError::TargetChanged);
        }
        if let Some(reason) = value.get("human").and_then(Value::as_str) {
            return Err(DriverError::HumanRequired(if reason == "captcha" {
                "captcha"
            } else {
                "login"
            }));
        }
        Ok(value)
    }

    /// Host-only operation. The server must obtain explicit user approval and
    /// fetch the secret directly from its credential store before calling this.
    pub fn fill_saved_login(
        &mut self,
        expected_origin: &str,
        approved: bool,
        secret: LoginSecret,
    ) -> Result<(), DriverError> {
        if !approved {
            return Err(DriverError::Refused("approval_required"));
        }
        let url = self.current_url()?;
        let actual = url::Url::parse(&url).map_err(|_| DriverError::Unavailable)?;
        if actual.origin().ascii_serialization() != expected_origin {
            return Err(DriverError::TargetChanged);
        }
        let fixture_http = cfg!(all(debug_assertions, feature = "fixture-sites"))
            && actual.host_str() == Some("127.0.0.1");
        if actual.scheme() != "https" && !cfg!(test) && !fixture_http {
            return Err(DriverError::Refused("insecure_login"));
        }
        self.protect_value(secret.username.clone());
        self.protect_value(secret.password.clone());
        let username = zeroize::Zeroizing::new(
            serde_json::to_string(&secret.username).map_err(|_| DriverError::Unavailable)?,
        );
        let password = zeroize::Zeroizing::new(
            serde_json::to_string(&secret.password).map_err(|_| DriverError::Unavailable)?,
        );
        let script = zeroize::Zeroizing::new(format!(
            r#"(()=>{{if(document.querySelector('iframe,input[autocomplete~=one-time-code]'))return 'verification';const p=[...document.querySelectorAll('input[type=password]')];const u=[...document.querySelectorAll('input[autocomplete=username],input[type=email],input[name=username],input[name=user]')];if(p.length!==1||u.length!==1||!p[0].form||p[0].form!==u[0].form)return 'verification';const form=p[0].form;if(new URL(form.action,location.href).origin!==location.origin||form.method.toLowerCase()!=='post')return 'verification';const set=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;set.call(u[0],{username});u[0].dispatchEvent(new Event('input',{{bubbles:true}}));set.call(p[0],{password});p[0].dispatchEvent(new Event('input',{{bubbles:true}}));form.requestSubmit();return 'ok'}})()"#,
            username = *username,
            password = *password
        ));
        match self.evaluate(&script, true)?.as_str() {
            Some("ok") => Ok(()),
            _ => Err(DriverError::HumanRequired("verification")),
        }
    }

    /// The host owns the approval callback; AI arguments must never set its result.
    pub fn authorize_action(
        &mut self,
        mode: Permission,
        mut ask: impl FnMut(&str) -> bool,
    ) -> Result<(), DriverError> {
        self.action_origin = None;
        if mode == Permission::Plan {
            return Err(DriverError::Refused("plan"));
        }
        let url = self.current_url()?;
        let parsed = url::Url::parse(&url).map_err(|_| DriverError::Unavailable)?;
        let host = parsed
            .host_str()
            .ok_or(DriverError::Unavailable)?
            .to_owned();
        let allow = mode == Permission::Automatic
            || *self
                .decisions
                .entry(host.clone())
                .or_insert_with(|| ask(&host));
        if allow {
            self.action_origin = Some(parsed.origin().ascii_serialization());
            Ok(())
        } else {
            Err(DriverError::Refused("denied"))
        }
    }

    fn consume_action(&mut self) -> Result<(), DriverError> {
        let expected = self
            .action_origin
            .take()
            .ok_or(DriverError::Refused("approval_required"))?;
        let current = self.current_url()?;
        let origin = url::Url::parse(&current)
            .map_err(|_| DriverError::Unavailable)?
            .origin()
            .ascii_serialization();
        if origin != expected {
            return Err(DriverError::TargetChanged);
        }
        Ok(())
    }

    fn redact(&self, value: &str) -> String {
        let redacted = self
            .login_secrets
            .iter()
            .fold(value.to_owned(), |text, secret| {
                text.replace(secret.as_str(), "[redacted]")
            });
        let mut output = String::new();
        let mut cursor = 0;
        while cursor < redacted.len() {
            let matched = self.one_time_hashes.iter().find(|(hash, len)| {
                redacted.get(cursor..cursor + *len).is_some_and(|part| {
                    let digest: [u8; 32] = Sha256::digest(part.as_bytes()).into();
                    digest == *hash
                })
            });
            if let Some((_, len)) = matched {
                output.push_str("[redacted]");
                cursor += len;
            } else if let Some(ch) = redacted[cursor..].chars().next() {
                output.push(ch);
                cursor += ch.len_utf8();
            } else {
                break;
            }
        }
        output
    }

    /// Host-only: register known sensitive values before exposing page results.
    pub fn protect_value(&mut self, mut value: String) {
        if value.is_empty()
            || self
                .login_secrets
                .iter()
                .any(|saved| saved.as_str() == value)
        {
            value.zeroize();
        } else {
            self.login_secrets.push(zeroize::Zeroizing::new(value));
        }
    }
}

impl BrowserEngine for HeadlessBrowser {
    fn navigate(&mut self, url: &str) -> Result<PageState, DriverError> {
        if !allowed_url(url) {
            return Err(DriverError::Blocked(String::new()));
        }
        let response = self.call("Page.navigate", json!({"url":url}))?;
        if response.get("errorText").is_some() {
            #[cfg(test)]
            if std::env::var_os("MOMSO_S159_URL").is_some() {
                let reason = response["errorText"].as_str().unwrap_or("unknown");
                if reason
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b":_-".contains(&c))
                {
                    eprintln!("S159 navigation_error={reason}");
                }
            }
            return Err(DriverError::Unavailable);
        }
        let expected = response
            .get("loaderId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        for attempt in 0..100 {
            thread::sleep(Duration::from_millis(50));
            if expected
                .as_deref()
                .is_some_and(|loader| self.current_frame().is_ok_and(|frame| frame.2 != loader))
            {
                continue;
            }
            // A title can arrive before the response body. Give the document a
            // bounded chance to finish parsing before returning the first read.
            if attempt < 40
                && self
                    .evaluate("document.readyState", false)
                    .is_ok_and(|state| state.as_str() == Some("loading"))
            {
                continue;
            }
            match self.read() {
                Ok(page) => return Ok(page),
                Err(DriverError::HumanRequired(reason)) => {
                    return Err(DriverError::HumanRequired(reason))
                }
                _ => {}
            }
        }
        Err(DriverError::Unavailable)
    }
    fn read(&mut self) -> Result<PageState, DriverError> {
        let value = self.evaluate(r#"(()=>{const s=window.__momso_driver_state||=({prefix:[...crypto.getRandomValues(new Uint32Array(4))].join('-'),next:0,byId:new Map(),byNode:new WeakMap()});const els=[...document.querySelectorAll('button,a,input,textarea,select,[role=button]')];return {origin:location.origin,title:document.title,text:document.body?.innerText?.slice(0,20000)||'',elements:els.map(e=>{let id=s.byNode.get(e);if(!id){id=s.prefix+':'+(++s.next);s.byNode.set(e,id);s.byId.set(id,e)}return {id,role:e.getAttribute('role')||e.tagName.toLowerCase(),name:e.getAttribute('aria-label')||e.innerText||e.getAttribute('placeholder')||'',kind:e.type==='password'?'password':(e.autocomplete||'').toLowerCase().includes('one-time-code')?'one_time_code':'normal'}})}})()"#, false)?;
        let elements = value
            .get("elements")
            .and_then(Value::as_array)
            .ok_or(DriverError::Unavailable)?
            .iter()
            .filter_map(|e| {
                Some(Element {
                    id: e.get("id")?.as_str()?.into(),
                    role: self.redact(e.get("role")?.as_str()?),
                    name: self.redact(e.get("name")?.as_str()?),
                    kind: match e.get("kind")?.as_str()? {
                        "password" => "password",
                        "one_time_code" => "one_time_code",
                        _ => "normal",
                    },
                })
            })
            .collect();
        let page = PageState {
            origin: value
                .get("origin")
                .and_then(Value::as_str)
                .ok_or(DriverError::Unavailable)?
                .into(),
            title: self.redact(value.get("title").and_then(Value::as_str).unwrap_or("")),
            text: self.redact(value.get("text").and_then(Value::as_str).unwrap_or("")),
            elements,
            data_provenance: "untrusted_page_data",
        };
        Ok(page)
    }
    fn click_checked(
        &mut self,
        id: &str,
        name: &str,
        origin: Option<&str>,
    ) -> Result<(), DriverError> {
        self.consume_action()?;
        let id = serde_json::to_string(id).map_err(|_| DriverError::Unavailable)?;
        let name = serde_json::to_string(name).map_err(|_| DriverError::Unavailable)?;
        let origin = serde_json::to_string(&origin).map_err(|_| DriverError::Unavailable)?;
        let script = format!(
            r#"(()=>{{if({origin}!==null&&location.origin!=={origin})return 'changed';const e=window.__momso_driver_state?.byId.get({id});if(!e||!e.isConnected)return 'changed';const n=e.getAttribute('aria-label')||e.innerText||e.getAttribute('placeholder')||'';if(n!=={name})return 'changed';e.click();return 'ok'}})()"#
        );
        if self.evaluate(&script, false)?.as_str() == Some("ok") {
            Ok(())
        } else {
            Err(DriverError::TargetChanged)
        }
    }
    fn fill_checked(
        &mut self,
        id: &str,
        text: &str,
        name: Option<&str>,
        origin: Option<&str>,
    ) -> Result<(), DriverError> {
        self.consume_action()?;
        let id = serde_json::to_string(id).map_err(|_| DriverError::Unavailable)?;
        let text = serde_json::to_string(text).map_err(|_| DriverError::Unavailable)?;
        let name = serde_json::to_string(&name).map_err(|_| DriverError::Unavailable)?;
        let origin = serde_json::to_string(&origin).map_err(|_| DriverError::Unavailable)?;
        let script = format!(
            r#"(()=>{{if({origin}!==null&&location.origin!=={origin})return 'changed';const e=window.__momso_driver_state?.byId.get({id});if(!e||!e.isConnected||!('value'in e))return 'changed';const n=e.getAttribute('aria-label')||e.innerText||e.getAttribute('placeholder')||'';if({name}!==null&&n!=={name})return 'changed';const t=(e.type||'').toLowerCase(),a=(e.autocomplete||'').toLowerCase().split(/\s+/);if(t==='password'||a.includes('current-password')||a.includes('new-password')||a.includes('one-time-code'))return 'protected';e.value={text};e.dispatchEvent(new Event('input',{{bubbles:true}}));return 'ok'}})()"#
        );
        match self.evaluate(&script, false)?.as_str() {
            Some("ok") => Ok(()),
            Some("protected") => Err(DriverError::Refused("protected")),
            _ => Err(DriverError::TargetChanged),
        }
    }
}

impl Drop for HeadlessBrowser {
    fn drop(&mut self) {
        let _ = self
            ._profile_guard
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()));
        let _ = self.send("Browser.close", json!({}), false);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct ServerBrowsers {
    executable: PathBuf,
    root: PathBuf,
    chats: HashMap<String, HeadlessBrowser>,
    closed_decisions: HashMap<String, HashMap<String, bool>>,
    closed_secrets: HashMap<String, Vec<zeroize::Zeroizing<String>>>,
    closed_code_hashes: HashMap<String, Vec<([u8; 32], usize)>>,
}
impl ServerBrowsers {
    pub fn new(executable: PathBuf, root: PathBuf) -> Self {
        Self {
            executable,
            root,
            chats: HashMap::new(),
            closed_decisions: HashMap::new(),
            closed_secrets: HashMap::new(),
            closed_code_hashes: HashMap::new(),
        }
    }
    pub fn open(&mut self, chat_id: &str) -> Result<&mut HeadlessBrowser, DriverError> {
        if chat_id.is_empty() {
            return Err(DriverError::Unavailable);
        }
        if !self.chats.contains_key(chat_id) {
            let digest = Sha256::digest(chat_id.as_bytes());
            let name = digest
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let mut browser = HeadlessBrowser::launch(&self.executable, &self.root.join(name))?;
            browser.decisions = self.closed_decisions.remove(chat_id).unwrap_or_default();
            browser.login_secrets = self.closed_secrets.remove(chat_id).unwrap_or_default();
            browser.one_time_hashes = self.closed_code_hashes.remove(chat_id).unwrap_or_default();
            self.chats.insert(chat_id.to_owned(), browser);
        }
        self.chats.get_mut(chat_id).ok_or(DriverError::Unavailable)
    }
    pub fn is_open(&self, chat_id: &str) -> bool {
        self.chats.contains_key(chat_id)
    }
    pub fn process_identity(&self, chat_id: &str) -> Option<(u32, u64)> {
        let pid = self.chats.get(chat_id)?.child.id();
        Some((pid, process_start_ticks(pid)?))
    }
    pub fn close(&mut self, chat_id: &str) {
        if let Some(mut browser) = self.chats.remove(chat_id) {
            self.closed_decisions
                .insert(chat_id.to_owned(), std::mem::take(&mut browser.decisions));
            self.closed_secrets.insert(
                chat_id.to_owned(),
                std::mem::take(&mut browser.login_secrets),
            );
            self.closed_code_hashes.insert(
                chat_id.to_owned(),
                std::mem::take(&mut browser.one_time_hashes),
            );
        }
    }
    pub fn forget_chat(&mut self, chat_id: &str) {
        self.chats.remove(chat_id);
        self.closed_decisions.remove(chat_id);
        self.closed_secrets.remove(chat_id);
        self.closed_code_hashes.remove(chat_id);
    }
}

fn process_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// Returns no value if a process disappears or its accounting is inaccessible.
pub fn process_tree_pss_kib((root, start): (u32, u64)) -> Option<u64> {
    if process_start_ticks(root)? != start {
        return None;
    }
    let mut processes = Vec::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        let Some(parent) = status
            .lines()
            .find_map(|line| line.strip_prefix("PPid:")?.trim().parse::<u32>().ok())
        else {
            continue;
        };
        processes.push((pid, parent));
    }
    let mut ids = std::collections::HashSet::from([root]);
    loop {
        let before = ids.len();
        for (pid, parent) in &processes {
            if ids.contains(parent) {
                ids.insert(*pid);
            }
        }
        if ids.len() == before {
            break;
        }
    }
    let mut total = 0u64;
    for pid in ids {
        let data = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
        let amount = data.lines().find_map(|line| {
            line.strip_prefix("Pss:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })?;
        total = total.checked_add(amount)?;
    }
    if process_start_ticks(root)? != start {
        return None;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    #[test]
    fn stale_cache_cleanup_preserves_login_and_skips_active_profile() {
        use super::*;
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("a".repeat(64));
        let default = profile.join("Default");
        let cache = default.join("Cache");
        let local = default.join("Local Storage");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(cache.join("cache"), b"temporary").unwrap();
        std::fs::write(default.join("Cookies"), b"login fixture").unwrap();
        std::fs::write(local.join("state"), b"local fixture").unwrap();
        let old = SystemTime::now() - Duration::from_secs(31 * 24 * 60 * 60);
        File::open(&cache)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        let dir = open_directory(&profile).unwrap();
        let active = profile_guard(&dir).unwrap();
        active
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        assert_eq!(prune_inactive_caches(tmp.path(), SystemTime::now()), 0);
        assert!(cache.exists());
        drop(active);
        let recent = profile_guard(&dir).unwrap();
        recent
            .set_times(std::fs::FileTimes::new().set_modified(SystemTime::now()))
            .unwrap();
        drop(recent);
        assert_eq!(prune_inactive_caches(tmp.path(), SystemTime::now()), 0);
        let inactive = profile_guard(&dir).unwrap();
        inactive
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        drop(inactive);
        std::fs::write(profile.join("SingletonLock"), b"active").unwrap();
        assert_eq!(prune_inactive_caches(tmp.path(), SystemTime::now()), 0);
        std::fs::remove_file(profile.join("SingletonLock")).unwrap();
        assert_eq!(prune_inactive_caches(tmp.path(), SystemTime::now()), 1);
        assert!(!cache.exists());
        assert_eq!(
            std::fs::read(default.join("Cookies")).unwrap(),
            b"login fixture"
        );
        assert_eq!(
            std::fs::read(local.join("state")).unwrap(),
            b"local fixture"
        );
    }

    #[test]
    fn stale_cache_cleanup_never_follows_profile_symlink() {
        use super::*;
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("Default/Cache")).unwrap();
        std::fs::write(outside.path().join("Default/Cache/file"), b"untouched").unwrap();
        symlink(outside.path(), tmp.path().join("b".repeat(64))).unwrap();
        assert_eq!(prune_inactive_caches(tmp.path(), SystemTime::now()), 0);
        assert!(outside.path().join("Default/Cache/file").exists());
    }

    #[test]
    #[ignore = "resource measurement needs an isolated Chromium and loopback fixture"]
    fn s155_browser_pss_measurement() {
        use std::io::{Read, Write};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = std::env::var("MOMSO_S159_URL")
            .unwrap_or_else(|_| format!("http://{}/", listener.local_addr().unwrap()));
        let count_limit = if std::env::var_os("MOMSO_S159_URL").is_some() {
            1
        } else {
            3
        };
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let heavy = std::env::var_os("MOMSO_S155_HEAVY").is_some();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut input = [0u8; 4096];
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = stream.read(&mut input);
                    let mut body =
                        String::from("<html><title>Fixture</title><body>resource fixture");
                    if heavy {
                        body.push_str("<table>");
                        for row in 0..3000 {
                            body.push_str(&format!(
                                "<tr><td>{row}</td><td>예약 수업 시간표</td><td>08:00</td></tr>"
                            ));
                        }
                        body.push_str("</table>");
                    }
                    body.push_str("</body></html>");
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        let executable = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("isolated browser");
        let root = tempfile::tempdir().unwrap();
        let mut browsers =
            super::ServerBrowsers::new(executable.into(), root.path().join("profiles"));
        let started = Instant::now();
        let mut completed = 0;
        for count in 1..=count_limit {
            let mem = std::fs::read_to_string("/proc/meminfo").unwrap();
            let available = mem
                .lines()
                .find_map(|line| {
                    line.strip_prefix("MemAvailable:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .unwrap();
            if available < 300_000 {
                println!(
                    "S155 scenario={} count={count} status=memory_low available_kib={available}",
                    if heavy { "heavy" } else { "light" }
                );
                break;
            }
            let browser = match browsers.open(&format!("fixture-{count}")) {
                Ok(browser) => browser,
                Err(error) => {
                    println!("S155 scenario={} count={count} status=launch_failed error={error:?} available_kib={available}", if heavy {"heavy"} else {"light"});
                    break;
                }
            };
            let oom_score =
                std::fs::read_to_string(format!("/proc/{}/oom_score_adj", browser.child.id()))
                    .unwrap();
            assert_eq!(oom_score.trim(), "500");
            let page_status = match super::BrowserEngine::navigate(browser, &url) {
                Ok(page) => {
                    if count_limit == 1 {
                        println!(
                            "S159 page_title={} text_chars={}",
                            serde_json::to_string(&page.title).unwrap(),
                            page.text.chars().count()
                        );
                    }
                    "opened"
                }
                Err(DriverError::HumanRequired(_)) if count_limit == 1 => "human_required",
                Err(error) => {
                    println!(
                        "S155 scenario={} count={count} status=navigate_failed error={error:?}",
                        if heavy { "heavy" } else { "light" }
                    );
                    break;
                }
            };
            std::thread::sleep(Duration::from_secs(if count_limit == 1 { 10 } else { 1 }));
            if count_limit == 1 {
                match super::BrowserEngine::read(
                    browsers.chats.get_mut(&format!("fixture-{count}")).unwrap(),
                ) {
                    Ok(page) => println!(
                        "S159 settled_title={} text_chars={}",
                        serde_json::to_string(&page.title).unwrap(),
                        page.text.chars().count()
                    ),
                    Err(error) => println!("S159 settled_read={error:?}"),
                }
            }
            let browser_pid = browsers.chats[&format!("fixture-{count}")].child.id();
            assert_eq!(
                std::fs::read_to_string(format!("/proc/{browser_pid}/oom_score_adj"))
                    .unwrap()
                    .trim(),
                "500"
            );
            let output = std::process::Command::new("ps")
                .args(["-eo", "pid=,ppid="])
                .output()
                .unwrap();
            let processes: Vec<(u32, u32)> = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
                })
                .collect();
            let mut ids: std::collections::HashSet<u32> = browsers
                .chats
                .values()
                .map(|browser| browser.child.id())
                .collect();
            loop {
                let before = ids.len();
                for (id, parent) in &processes {
                    if ids.contains(parent) {
                        ids.insert(*id);
                    }
                }
                if ids.len() == before {
                    break;
                }
            }
            let mut pss = 0u64;
            let mut read_count = 0;
            for id in &ids {
                if let Ok(rollup) = std::fs::read_to_string(format!("/proc/{id}/smaps_rollup")) {
                    if let Some(value) = rollup.lines().find_map(|line| {
                        line.strip_prefix("Pss:")?
                            .split_whitespace()
                            .next()?
                            .parse::<u64>()
                            .ok()
                    }) {
                        pss += value;
                        read_count += 1;
                    }
                }
            }
            let mem = std::fs::read_to_string("/proc/meminfo").unwrap();
            let available = mem
                .lines()
                .find_map(|line| {
                    line.strip_prefix("MemAvailable:")?
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()
                })
                .unwrap();
            println!("S155 scenario={} count={count} status={page_status} processes={} pss_read={} pss_kib={pss} available_kib={available} elapsed_ms={}", if heavy {"heavy"} else {"light"}, ids.len(), read_count, started.elapsed().as_millis());
            completed = count;
        }
        drop(browsers);
        stop.store(true, Ordering::SeqCst);
        worker.join().unwrap();
        assert_eq!(
            completed, count_limit,
            "browser measurement did not finish all cases"
        );
    }
    #[test]
    #[ignore = "requires isolated headless Chromium and a loopback fixture"]
    fn browser_resource_usage_one_and_three_chats() {
        use std::io::{Read, Write};
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let server = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::SeqCst) {
                if let Ok((mut stream, _)) = listener.accept() {
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(1)));
                    let mut input = [0u8; 4096];
                    let _ = stream.read(&mut input);
                    let body = "<html><title>Fixture</title><body>resource fixture</body></html>";
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        });
        let executable = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("isolated test browser");
        let root = tempfile::tempdir().unwrap();
        let mut browsers =
            super::ServerBrowsers::new(executable.into(), root.path().join("profiles"));
        let started = std::time::Instant::now();
        for count in 1..=3 {
            let browser = browsers.open(&format!("fixture-{count}")).unwrap();
            let page = super::BrowserEngine::navigate(browser, &url).unwrap();
            assert!(page.text.contains("resource fixture"));
            if count == 1 || count == 3 {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let output = std::process::Command::new("ps")
                    .args(["-eo", "pid=,ppid=,rss="])
                    .output()
                    .unwrap();
                assert!(output.status.success());
                let processes: Vec<(u32, u32, u64)> = String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .filter_map(|line| {
                        let mut fields = line.split_whitespace();
                        Some((
                            fields.next()?.parse().ok()?,
                            fields.next()?.parse().ok()?,
                            fields.next()?.parse().ok()?,
                        ))
                    })
                    .collect();
                let mut ids: std::collections::HashSet<u32> = browsers
                    .chats
                    .values()
                    .map(|browser| browser.child.id())
                    .collect();
                loop {
                    let before = ids.len();
                    for (id, parent, _) in &processes {
                        if ids.contains(parent) {
                            ids.insert(*id);
                        }
                    }
                    if ids.len() == before {
                        break;
                    }
                }
                let rss_kib: u64 = processes
                    .iter()
                    .filter(|(id, _, _)| ids.contains(id))
                    .map(|(_, _, rss)| rss)
                    .sum();
                assert!(rss_kib > 0);
                println!(
                    "S144 chats={count} processes={} rss_kib={rss_kib} elapsed_ms={}",
                    ids.len(),
                    started.elapsed().as_millis()
                );
            }
        }
        drop(browsers);
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
    }
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };

    #[test]
    fn address_guard_rejects_private_and_non_web_targets() {
        for url in [
            "file:///etc/passwd",
            "http://localhost/",
            "http://10.0.0.1/",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "https://user@example.com/",
        ] {
            assert!(!allowed_url(url), "{url}");
        }
        assert!(allowed_url("https://[2606:4700:4700::1111]/"));
        assert!(allowed_request("wss://[2606:4700:4700::1111]/"));
        assert!(!allowed_request("ws://10.0.0.1/"));
    }

    #[test]
    fn provider_permission_names_match_desktop() {
        assert_eq!(
            Permission::from_provider_mode("codex", "Full access"),
            Permission::Automatic
        );
        assert_eq!(
            Permission::from_provider_mode("codex", "Approve for me"),
            Permission::AutoReview
        );
        assert_eq!(
            Permission::from_provider_mode("claude", "auto"),
            Permission::AutoReview
        );
        assert_eq!(
            Permission::from_provider_mode("gemini", "plan"),
            Permission::Plan
        );
        assert_eq!(
            Permission::from_provider_mode("gemini", "default"),
            Permission::Ask
        );
    }

    #[test]
    fn profile_symlink_does_not_change_another_directory() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = tempfile::tempdir().unwrap();
        let other = root.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("link");
        symlink(&other, &link).unwrap();
        assert!(matches!(
            HeadlessBrowser::launch(Path::new("/missing"), &link),
            Err(DriverError::Refused("invalid_profile"))
        ));
        assert_eq!(
            std::fs::metadata(other).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    #[ignore = "requires an isolated headless Chromium executable"]
    fn delayed_body_is_read_after_document_parsing() {
        use std::io::{Read, Write};
        let executable = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("isolated test browser");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let first = "<html><head><title>Delayed</title></head>";
            let last = "<body>Delayed lesson content</body></html>";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{first}", first.len() + last.len()).unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(700));
            stream.write_all(last.as_bytes()).unwrap();
        });
        let profile = tempfile::tempdir().unwrap();
        let mut browser = HeadlessBrowser::launch(Path::new(&executable), profile.path()).unwrap();
        let page = browser.navigate(&format!("http://{address}/")).unwrap();
        assert!(page.text.contains("Delayed lesson content"), "{page:?}");
        server.join().unwrap();
    }

    #[test]
    #[ignore = "requires an isolated headless Chromium executable"]
    fn linux_headless_chats_approval_saved_login_and_restart() {
        let executable = std::env::var_os("MOMSO_TEST_CHROMIUM").expect("isolated test browser");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let registrations = Arc::new(AtomicUsize::new(0));
        let logins = Arc::new(AtomicUsize::new(0));
        let credential_match = Arc::new(AtomicBool::new(false));
        let otp_match = Arc::new(AtomicBool::new(false));
        let inner_frame_loaded = Arc::new(AtomicBool::new(false));
        let server_stop = stop.clone();
        let server_registrations = registrations.clone();
        let server_logins = logins.clone();
        let server_credential_match = credential_match.clone();
        let server_otp_match = otp_match.clone();
        let server_inner_frame_loaded = inner_frame_loaded.clone();
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut buffer = [0; 8192];
                let mut incoming = Vec::new();
                while let Ok(n) = stream.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    incoming.extend_from_slice(&buffer[..n]);
                    if let Some(end) = incoming.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&incoming[..end]);
                        let length = header
                            .lines()
                            .find_map(|line| {
                                line.split_once(':').and_then(|(key, value)| {
                                    key.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                            })
                            .unwrap_or(0);
                        if incoming.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8_lossy(&incoming);
                let (body, cookie) = if request.starts_with("GET /login ") {
                    ("<html><title>Login</title><form method='post' action='/login'><input name='user'><input name='pw' type='password'><button>Sign in</button></form></html>", "")
                } else if request.starts_with("GET /otp ") {
                    (
                        "<html><title>Verify</title><input autocomplete='one-time-code'></html>",
                        "",
                    )
                } else if request.starts_with("GET /otp-form ") {
                    ("<html><title>Verify</title><form method='post' action='/verify'><input name='code' autocomplete='one-time-code'><button>verify</button></form></html>", "")
                } else if request.starts_with("GET /captcha-form ") {
                    (
                        "<html><body><div class='g-recaptcha'></div></body></html>",
                        "",
                    )
                } else if request.starts_with("GET /passkey-form ") {
                    (
                        "<html><body><input autocomplete='webauthn'></body></html>",
                        "",
                    )
                } else if request.starts_with("GET /echo-code ") {
                    ("<html><body>code 123456</body></html>", "")
                } else if request.starts_with("POST /verify ") {
                    server_otp_match.store(request.contains("code=123456"), Ordering::SeqCst);
                    (
                        "<html><title>Verified</title><body>verified 123456</body></html>",
                        "Set-Cookie: fixture=ok; Max-Age=3600; Path=/\r\n",
                    )
                } else if request.starts_with("POST /login ") {
                    server_credential_match.store(
                        request.contains("user=user") && request.contains("pw=secret"),
                        Ordering::SeqCst,
                    );
                    server_logins.fetch_add(1, Ordering::SeqCst);
                    (
                        "<html><title>Logged in</title><body>logged in secret user</body></html>",
                        "Set-Cookie: fixture=ok; Max-Age=3600; Path=/\r\n",
                    )
                } else if request.starts_with("POST /register ") {
                    server_registrations.fetch_add(1, Ordering::SeqCst);
                    ("ok", "")
                } else if request.starts_with("GET /delayed ") {
                    ("late-result", "")
                } else if request.starts_with("GET /popup ") {
                    (
                        "<html><title>Popup</title><body>new-window-content<a href='/nested' target='_blank'>nested</a></body></html>",
                        "",
                    )
                } else if request.starts_with("GET /nested ") {
                    (
                        "<html><title>Nested</title><body>nested-window-content</body></html>",
                        "",
                    )
                } else if request.starts_with("GET /framed ") {
                    ("<html><title>Framed</title><body>framed-page<iframe src='/frame'></iframe></body></html>", "")
                } else if request.starts_with("GET /frame ") {
                    ("<html><body>frame-content<iframe src='/inner-frame'></iframe></body></html>", "")
                } else if request.starts_with("GET /inner-frame ") {
                    server_inner_frame_loaded.store(true, Ordering::SeqCst);
                    ("<html><body>nested-frame-content</body></html>", "")
                } else if request.to_ascii_lowercase().contains("cookie: fixture=ok") {
                    (
                        "<html><title>Remembered</title><body>remembered secret user</body></html>",
                        "",
                    )
                } else {
                    ("<html><title>Classes</title><body><button aria-label='menu'>menu</button><input placeholder='search'><button aria-label='register' onclick=\"fetch('/register',{method:'POST'})\">register</button><button aria-label='late' onclick=\"setTimeout(()=>fetch('/delayed').then(r=>r.text()).then(t=>document.querySelector('#result').textContent=t),100)\">late</button><a href='/popup' target='_blank'>popup</a><p>schedule</p><p id='result'></p></body></html>", "")
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n{cookie}Connection: close\r\n\r\n{body}", body.len());
            }
        });
        let temp = tempfile::tempdir().unwrap();
        let mut browsers = ServerBrowsers::new(executable.into(), temp.path().join("profiles"));
        let base = format!("http://{address}");
        let page = browsers
            .open("room-a")
            .unwrap()
            .navigate(&format!("{base}/"))
            .unwrap();
        assert!(page.text.contains("schedule"));
        let menu = page.elements.iter().find(|e| e.name == "menu").unwrap();
        let browser = browsers.open("room-a").unwrap();
        assert_eq!(
            browser.authorize_action(Permission::Ask, |_| false),
            Err(DriverError::Refused("denied"))
        );
        assert_eq!(
            browser.click_checked(&menu.id, &menu.name, Some(&base)),
            Err(DriverError::Refused("approval_required"))
        );
        assert_eq!(registrations.load(Ordering::SeqCst), 0);
        assert_eq!(
            browser.authorize_action(Permission::Ask, |_| true),
            Err(DriverError::Refused("denied"))
        );
        browser
            .authorize_action(Permission::Automatic, |_| panic!("automatic must not ask"))
            .unwrap();
        browser
            .click_checked(&menu.id, &menu.name, Some(&base))
            .unwrap();
        let page = browser.read().unwrap();
        let search = page.elements.iter().find(|e| e.name == "search").unwrap();
        browser
            .authorize_action(Permission::Automatic, |_| panic!("automatic must not ask"))
            .unwrap();
        browser
            .fill_checked(&search.id, "class", Some("search"), Some(&base))
            .unwrap();
        let page = browser.read().unwrap();
        let register = page.elements.iter().find(|e| e.name == "register").unwrap();
        browser
            .authorize_action(Permission::Automatic, |_| panic!("automatic must not ask"))
            .unwrap();
        browser
            .click_checked(&register.id, "register", Some(&base))
            .unwrap();
        for _ in 0..100 {
            if registrations.load(Ordering::SeqCst) == 1 {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(registrations.load(Ordering::SeqCst), 1);
        let page = browser.read().unwrap();
        let late = page.elements.iter().find(|e| e.name == "late").unwrap();
        browser
            .authorize_action(Permission::Automatic, |_| false)
            .unwrap();
        browser
            .click_checked(&late.id, "late", Some(&base))
            .unwrap();
        thread::sleep(Duration::from_secs(1));
        assert!(browser.read().unwrap().text.contains("late-result"));
        let popup_page = browser.read().unwrap();
        let popup = popup_page
            .elements
            .iter()
            .find(|e| e.name == "popup")
            .unwrap();
        browser
            .authorize_action(Permission::Automatic, |_| false)
            .unwrap();
        browser
            .click_checked(&popup.id, "popup", Some(&base))
            .unwrap();
        let mut popup_seen = false;
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(50));
            if browser
                .read()
                .is_ok_and(|page| page.text.contains("new-window-content"))
            {
                popup_seen = true;
                break;
            }
        }
        assert!(popup_seen);
        let page = browser.read().unwrap();
        let nested = page.elements.iter().find(|e| e.name == "nested").unwrap();
        browser
            .authorize_action(Permission::Automatic, |_| false)
            .unwrap();
        browser
            .click_checked(&nested.id, "nested", Some(&base))
            .unwrap();
        let mut nested_seen = false;
        for _ in 0..100 {
            thread::sleep(Duration::from_millis(50));
            if browser
                .read()
                .is_ok_and(|p| p.text.contains("nested-window-content"))
            {
                nested_seen = true;
                break;
            }
        }
        assert!(nested_seen);
        let framed = browser.navigate(&format!("{base}/framed")).unwrap();
        assert_eq!(framed.title, "Framed");
        for _ in 0..100 {
            if inner_frame_loaded.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        assert!(inner_frame_loaded.load(Ordering::SeqCst));
        assert_eq!(browser.read().unwrap().title, "Framed");
        assert!(matches!(
            browser.navigate(&format!("{base}/otp-form")),
            Err(DriverError::HumanRequired("login"))
        ));
        let (old_url, old_loader) = browser.one_time_code_document().unwrap();
        browser.navigate(&format!("{base}/framed")).unwrap();
        assert_eq!(
            browser.submit_one_time_code(
                &old_url,
                &old_loader,
                zeroize::Zeroizing::new("123456".into())
            ),
            Err(DriverError::TargetChanged)
        );
        assert!(!otp_match.load(Ordering::SeqCst));
        assert!(matches!(
            browser.navigate(&format!("{base}/login")),
            Err(DriverError::HumanRequired("login"))
        ));
        assert_eq!(browser.human_login_request("login").unwrap().origin, base);
        assert_eq!(
            browser.fill_saved_login(
                &base,
                false,
                LoginSecret::new("user".into(), "secret".into())
            ),
            Err(DriverError::Refused("approval_required"))
        );
        assert_eq!(
            browser.fill_saved_login(
                "http://wrong.example",
                true,
                LoginSecret::new("user".into(), "secret".into())
            ),
            Err(DriverError::TargetChanged)
        );
        browser
            .fill_saved_login(
                &base,
                true,
                LoginSecret::new("user".into(), "secret".into()),
            )
            .unwrap();
        for _ in 0..100 {
            if logins.load(Ordering::SeqCst) == 1 {
                break;
            }
            let _ = browser.read();
            thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(logins.load(Ordering::SeqCst), 1);
        assert!(credential_match.load(Ordering::SeqCst));
        let mut login_seen = false;
        for _ in 0..100 {
            if let Ok(page) = browser.read() {
                if page.title == "Logged in" {
                    assert!(!page.text.contains("secret") && !page.text.contains("user"));
                    login_seen = true;
                    break;
                }
            }
            thread::sleep(Duration::from_millis(30));
        }
        assert!(login_seen);
        let other = browsers
            .open("room-b")
            .unwrap()
            .navigate(&format!("{base}/"))
            .unwrap();
        assert_eq!(other.title, "Classes");
        let other_menu = other.elements.iter().find(|e| e.name == "menu").unwrap();
        let other_browser = browsers.open("room-b").unwrap();
        other_browser
            .authorize_action(Permission::Ask, |_| true)
            .unwrap();
        other_browser
            .click_checked(&other_menu.id, &other_menu.name, Some(&base))
            .unwrap();
        other_browser
            .authorize_action(Permission::Ask, |_| panic!("site must not ask twice"))
            .unwrap();
        assert!(matches!(
            other_browser.navigate(&format!("{base}/otp")),
            Err(DriverError::HumanRequired("login"))
        ));
        browsers.close("room-a");
        assert_eq!(
            browsers
                .open("room-a")
                .unwrap()
                .navigate(&format!("{base}/"))
                .unwrap()
                .title,
            "Remembered"
        );
        let redacted = browsers.open("room-a").unwrap().read().unwrap();
        assert!(!redacted.text.contains("secret") && !redacted.text.contains("user"));
        assert_eq!(
            browsers
                .open("room-a")
                .unwrap()
                .authorize_action(Permission::Ask, |_| panic!(
                    "previous denial must be remembered"
                )),
            Err(DriverError::Refused("denied"))
        );
        struct FakeApproval {
            calls: usize,
            allow: bool,
            login_calls: usize,
        }
        impl crate::tools::ApprovalChannel for FakeApproval {
            fn approve_site(&mut self, _: &str, _: &str) -> bool {
                self.calls += 1;
                self.allow
            }
            fn approve_login(&mut self, _: &str, _: &str) -> bool {
                self.login_calls += 1;
                true
            }
            fn human_login(&mut self, _: &str, _: &str) {}
            fn one_time_code(&mut self, _: &str, _: &str) -> Option<zeroize::Zeroizing<String>> {
                Some(zeroize::Zeroizing::new("123456".into()))
            }
        }
        struct FakeVault;
        impl crate::tools::CredentialSource for FakeVault {
            fn protected_values(&mut self) -> Result<Vec<String>, crate::tools::CredentialError> {
                Ok(vec!["secret".into(), "user".into()])
            }
            fn login_for_origin(
                &mut self,
                _: &str,
                _: &str,
            ) -> Result<Option<LoginSecret>, crate::tools::CredentialError> {
                Ok(Some(LoginSecret::new("user".into(), "secret".into())))
            }
        }
        let mut tools = crate::tools::BrowserTools::new(
            ServerBrowsers::new(
                std::env::var_os("MOMSO_TEST_CHROMIUM").unwrap().into(),
                temp.path().join("tool-profiles"),
            ),
            FakeApproval {
                calls: 0,
                allow: true,
                login_calls: 0,
            },
            FakeVault,
        );
        assert_eq!(
            tools.dispatch("tool-room", Permission::Ask, "browser_open", json!({}))["status"],
            "ok"
        );
        let page = tools.dispatch(
            "tool-room",
            Permission::Ask,
            "browser_navigate",
            json!({"url":base}),
        );
        assert_eq!(page["status"], "page");
        let menu = page["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "menu")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        assert_eq!(
            tools.dispatch(
                "tool-room",
                Permission::Ask,
                "browser_click",
                json!({"elementId":menu})
            )["status"],
            "ok"
        );
        assert_eq!(tools.approval.calls, 1);
        let read = tools.dispatch("tool-room", Permission::Ask, "browser_read", json!({}));
        assert_eq!(read["status"], "page");
        let search = read["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "search")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        assert_eq!(
            tools.dispatch(
                "tool-room",
                Permission::Ask,
                "browser_fill",
                json!({"elementId":search,"text":"class"})
            )["status"],
            "ok"
        );
        assert_eq!(tools.approval.calls, 1);
        tools.approval.allow = false;
        let denied_page = tools.dispatch(
            "denied-room",
            Permission::Ask,
            "browser_navigate",
            json!({"url":base}),
        );
        let register = denied_page["elements"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == "register")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        assert_eq!(
            tools.dispatch(
                "denied-room",
                Permission::Ask,
                "browser_click",
                json!({"elementId":register})
            )["status"],
            "denied"
        );
        assert_eq!(registrations.load(Ordering::SeqCst), 1);
        assert!(tools.take_activity("other-room").is_empty());
        let activity = tools.take_activity("tool-room");
        assert!(activity.iter().any(|event| event.contains("menu")));
        assert!(activity.iter().any(|event| event.contains("Classes")));
        assert_eq!(
            tools.dispatch(
                "tool-room",
                Permission::Ask,
                "browser_navigate",
                json!({"url":format!("{base}/login")})
            )["status"],
            "human_required"
        );
        assert_eq!(
            tools.dispatch(
                "tool-room",
                Permission::Ask,
                "browser_request_human_login",
                json!({"reason":"login"})
            )["status"],
            "human_required"
        );
        assert_eq!(
            tools.dispatch(
                "tool-room",
                Permission::Ask,
                "browser_request_saved_login",
                json!({})
            )["status"],
            "ok"
        );
        drop(tools);
        struct SelectFirst;
        impl crate::vault::LoginSelector for SelectFirst {
            fn select(
                &mut self,
                _: &str,
                choices: &[momso_saved_login_core::linux::LoginInfo],
            ) -> Option<String> {
                choices.first().map(|entry| entry.id.clone())
            }
        }
        let vault = momso_saved_login_core::linux::LinuxVault::new(
            &temp.path().join("vault"),
            age::x25519::Identity::generate(),
        )
        .unwrap();
        vault
            .save_from_phone(momso_saved_login_core::linux::PhoneSaveRequest {
                site_url: format!("{base}/login"),
                label: "fixture".into(),
                username: "user".into(),
                password: "secret".into(),
                ask_before_use: true,
                login_path: Some("/login".into()),
                default_for_path: true,
            })
            .unwrap();
        let source = crate::vault::VaultCredentials {
            vault,
            selector: SelectFirst,
        };
        let mut real = crate::tools::BrowserTools::new(
            ServerBrowsers::new(
                std::env::var_os("MOMSO_TEST_CHROMIUM").unwrap().into(),
                temp.path().join("vault-browser"),
            ),
            FakeApproval {
                calls: 0,
                allow: true,
                login_calls: 0,
            },
            source,
        );
        assert_eq!(
            real.dispatch(
                "vault-room",
                Permission::Ask,
                "browser_navigate",
                json!({"url":format!("{base}/login")})
            )["status"],
            "human_required"
        );
        assert_eq!(
            real.dispatch(
                "vault-room",
                Permission::Ask,
                "browser_request_saved_login",
                json!({})
            )["status"],
            "ok"
        );
        let mut vault_login_seen = false;
        for _ in 0..100 {
            let value = real.dispatch("vault-room", Permission::Ask, "browser_read", json!({}));
            let output = value.to_string();
            assert!(!output.contains("secret") && !output.contains("user"));
            if value["title"] == "Logged in" {
                vault_login_seen = true;
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        assert!(vault_login_seen);
        assert!(real.browser_is_open("vault-room"));
        assert!(real.close_idle_browser("vault-room"));
        assert!(!real.browser_is_open("vault-room"));
        let restored = real.dispatch(
            "vault-room",
            Permission::Ask,
            "browser_navigate",
            json!({"url":base}),
        );
        assert_eq!(restored["title"], "Remembered");
        assert_eq!(
            real.dispatch(
                "vault-room",
                Permission::Ask,
                "browser_navigate",
                json!({"url":format!("{base}/otp-form")})
            )["status"],
            "human_required"
        );
        let result = real.dispatch(
            "vault-room",
            Permission::Ask,
            "browser_request_human_login",
            json!({"reason":"login"}),
        );
        assert_eq!(result["status"], "ok");
        for _ in 0..100 {
            if otp_match.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        assert!(otp_match.load(Ordering::SeqCst));
        let mut verified_seen = false;
        for _ in 0..100 {
            let value = real.dispatch("vault-room", Permission::Ask, "browser_read", json!({}));
            let encoded = value.to_string();
            assert!(
                !encoded.contains("123456")
                    && !encoded.contains("secret")
                    && !encoded.contains("user")
            );
            if value["title"] == "Verified" {
                verified_seen = true;
                break;
            }
            thread::sleep(Duration::from_millis(30));
        }
        assert!(verified_seen);
        assert_eq!(real.approval.login_calls, 1);
        real.browsers.close("vault-room");
        let digest = Sha256::digest(b"vault-room");
        let name: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        let profiles = temp.path().join("vault-browser");
        let cache = profiles.join(&name).join("Default/Cache");
        std::fs::create_dir_all(&cache).unwrap();
        let old = SystemTime::now() - Duration::from_secs(31 * 24 * 60 * 60);
        File::open(&cache)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        File::open(profiles.join(&name).join(".momso-profile.lock"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
        assert!(prune_inactive_caches(&profiles, SystemTime::now()) >= 1);
        let remembered = real.dispatch(
            "vault-room",
            Permission::Ask,
            "browser_navigate",
            json!({"url":format!("{base}/")}),
        );
        assert_eq!(remembered["title"], "Remembered");
        assert_eq!(real.approval.login_calls, 1);
        let echo = real.dispatch(
            "vault-room",
            Permission::Ask,
            "browser_navigate",
            json!({"url":format!("{base}/echo-code")}),
        );
        assert_eq!(echo["status"], "page");
        assert!(!echo.to_string().contains("123456"));
        let activity = real.take_activity("vault-room").join(" ");
        assert!(
            !activity.contains("123456")
                && !activity.contains("secret")
                && !activity.contains("user")
        );
        for (path, reason) in [("captcha-form", "captcha"), ("passkey-form", "passkey")] {
            real.dispatch(
                "vault-room",
                Permission::Ask,
                "browser_navigate",
                json!({"url":format!("{base}/{path}")}),
            );
            let result = real.dispatch(
                "vault-room",
                Permission::Ask,
                "browser_request_human_login",
                json!({"reason":"login"}),
            );
            assert_eq!(result["status"], "human_required");
            assert_eq!(result["reason"], reason);
        }
        drop(real);
        drop(browsers);
        stop.store(true, Ordering::Release);
        server.join().unwrap();
    }
}
