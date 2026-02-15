// Prevents additional console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod whatsapp_client;

use std::sync::Arc;
use whatsapp_client::WhatsAppState;

fn main() {
    let whatsapp_state = Arc::new(WhatsAppState::new());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(whatsapp_state)
        .invoke_handler(tauri::generate_handler![
            whatsapp_client::init_whatsapp,
            whatsapp_client::is_bot_ready,
            whatsapp_client::send_message,
            whatsapp_client::send_media_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}