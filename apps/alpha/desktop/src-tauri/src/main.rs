// Prevents an extra console window in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    #[cfg(windows)]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--momso-update-supervisor")
    {
        momso_desktop_lib::run_update_supervisor(args.collect());
        return;
    }
    #[cfg(windows)]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--momso-update-recover")
    {
        momso_desktop_lib::run_update_recovery();
        return;
    }
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--momso-s5b-window-probe")
    {
        momso_desktop_lib::s5b_window_probe::run();
        return;
    }
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--momso-g83-test-probe")
    {
        momso_desktop_lib::g61_test_probe::run_g83();
        return;
    }
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--momso-g62-test-probe")
    {
        momso_desktop_lib::g61_test_probe::run_g62();
        return;
    }
    #[cfg(all(windows, debug_assertions, feature = "g61-test-probe"))]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--momso-g61-test-probe")
    {
        momso_desktop_lib::g61_test_probe::run();
        return;
    }
    if first.is_some_and(|arg| arg == "--momso-mcp-stdio") {
        let pipe = match args.next() {
            Some(flag) if flag == "--momso-setup-pipe" => {
                args.next().and_then(|v| v.into_string().ok())
            }
            _ => None,
        };
        let _ = momso_desktop_lib::serve_mcp_stdio(pipe);
        return;
    }
    momso_desktop_lib::run()
}
