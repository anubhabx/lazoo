mod client;

use lazoo_protocol::DesktopSnapshot;

use client::DesktopFailure;

#[tauri::command]
async fn load_desktop_snapshot() -> Result<DesktopSnapshot, DesktopFailure> {
    client::fetch_snapshot().await
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![load_desktop_snapshot])
        .run(tauri::generate_context!())
        .expect("Lazoo desktop runtime failed");
}
