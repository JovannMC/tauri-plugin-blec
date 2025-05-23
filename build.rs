const COMMANDS: &[&str] = &[
    "scan",
    "stop_scan",
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
