// Prevents an extra console window in Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|arg| arg == "--momso-mcp-stdio") {
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
