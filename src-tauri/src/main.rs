#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--clipboard-snapshot") {
        flow_lib::clipboard_snapshot::run_helper();
        return;
    }
    flow_lib::run();
}
