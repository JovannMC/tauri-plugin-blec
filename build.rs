const COMMANDS: &[&str] = &[
    "start_scan",
    "stop_scan",
    "start_scan_stream",
    "stop_scan_stream",
    "connect",
    "disconnect",
    "connection_state",
    "send",
    "send_string",
    "read",
    "read_string",
    "subscribe",
    "subscribe_string",
    "unsubscribe",
    "scanning_state",
    "check_permissions",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .build();
}
