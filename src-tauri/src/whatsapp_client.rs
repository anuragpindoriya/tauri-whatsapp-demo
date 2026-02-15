use std::sync::Arc;
use tauri::{Emitter, State, Window, Manager};
use tokio::sync::Mutex;
use whatsapp_rust::bot::Bot;
use whatsapp_rust::store::SqliteStore;
use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
use whatsapp_rust_ureq_http_client::UreqHttpClient;
use serde::Serialize;

// Import types from whatsapp_rust with correct paths
use whatsapp_rust::types::events::Event;
use whatsapp_rust::Jid;
use whatsapp_rust::waproto::whatsapp as wa;
use whatsapp_rust::download::MediaType;

pub struct WhatsAppState {
    bot: Arc<Mutex<Option<Bot>>>,
    is_authenticated: Arc<Mutex<bool>>,
    is_ready: Arc<Mutex<bool>>, // New: Track if bot is fully ready
}

impl WhatsAppState {
    pub fn new() -> Self {
        Self {
            bot: Arc::new(Mutex::new(None)),
            is_authenticated: Arc::new(Mutex::new(false)),
            is_ready: Arc::new(Mutex::new(false)),
        }
    }
}

// Serializable QR code event for frontend
#[derive(Clone, Serialize)]
struct QrCodeEvent {
    code: String,
}

// Tauri Command: Initialize WhatsApp connection
#[tauri::command]
pub async fn init_whatsapp(
    window: Window,
    state: State<'_, Arc<WhatsAppState>>,
) -> Result<(), String> {
    // Get app data directory (outside of src-tauri to avoid rebuild loops)
    let app_handle = window.app_handle();
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    
    // Create the directory if it doesn't exist
    std::fs::create_dir_all(&app_data_dir).map_err(|e| e.to_string())?;
    
    // Database path in app data directory
    let db_path = app_data_dir.join("whatsapp.db");
    let db_path_str = db_path.to_string_lossy().to_string();
    
    println!("Using database path: {}", db_path_str);

    let backend = SqliteStore::new(&db_path_str)
        .await
        .map_err(|e| e.to_string())?;

    let window_clone = window.clone();
    let state_clone = state.inner().clone();
    let state_for_bot = state.inner().clone(); // Clone again for bot storage
    
    tokio::spawn(async move {
        let bot_result = Bot::builder()
            .with_backend(Arc::new(backend))
            .with_transport_factory(TokioWebSocketTransportFactory::new())
            .with_http_client(UreqHttpClient::new())
            .on_event(move |event, _client| {
                let window = window_clone.clone();
                let state = state_clone.clone();
                
                async move {
                    match event {
                        // Emit QR Code to frontend
                        Event::PairingQrCode { code, .. } => {
                            println!("QR Code generated");
                            let _ = window.emit("qr-code", QrCodeEvent { code });
                        }
                        
                        // Authentication successful
                        Event::PairSuccess(_) => {
                            println!("Pair success event received");
                            *state.is_authenticated.lock().await = true;
                            let _ = window.emit("auth-success", ());
                        }
                        
                        // Authentication connected - Bot is fully ready
                        Event::Connected(_) => {
                            println!("Connected event received - Bot is fully ready");
                            *state.is_authenticated.lock().await = true;
                            *state.is_ready.lock().await = true;
                            let _ = window.emit("auth-success", ());
                        }
                        
                        // Log other events for debugging
                        Event::LoggedOut(_) => {
                            println!("Logged out event received");
                            *state.is_authenticated.lock().await = false;
                            *state.is_ready.lock().await = false;
                        }
                        
                        Event::Message(_msg, info) => {
                            println!("Message received from: {:?}", info.source.sender);
                        }
                        
                        _ => {
                            // println!("Other event received: {:?}", event);
                        }
                    }
                }
            })
            .build()
            .await;

        match bot_result {
            Ok(mut bot) => {
                println!("Bot built successfully, starting...");
                // Start the bot
                match bot.run().await {
                    Ok(handle) => {
                        println!("Bot started successfully");
                        *state_for_bot.bot.lock().await = Some(bot);
                        // Keep the task alive
                        let _ = handle.await;
                    }
                    Err(e) => {
                        eprintln!("Failed to run bot: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to build bot: {}", e);
            }
        }
        
        Ok::<_, String>(())
    });

    Ok(())
}

// Tauri Command: Check if bot is ready
#[tauri::command]
pub async fn is_bot_ready(
    state: State<'_, Arc<WhatsAppState>>,
) -> Result<bool, String> {
    let is_ready = *state.is_ready.lock().await;
    Ok(is_ready)
}

// Tauri Command: Send text message
#[tauri::command]
pub async fn send_message(
    contact: String,
    message: String,
    state: State<'_, Arc<WhatsAppState>>,
) -> Result<String, String> {
    // Check if bot is ready
    let is_ready = *state.is_ready.lock().await;
    if !is_ready {
        return Err("WhatsApp is not ready yet. Please wait for connection to complete.".to_string());
    }

    // Get client reference and release the bot lock immediately
    let client = {
        let bot_guard = state.bot.lock().await;
        let bot = bot_guard.as_ref().ok_or("WhatsApp not initialized")?;
        bot.client()
    };
    
    // Parse contact to JID format (remove any + or spaces)
    let clean_contact = contact.replace(['+', ' ', '-'], "");
    println!("Sending message to contact: {}", clean_contact);
    
    let jid = Jid::new(&clean_contact, "s.whatsapp.net");
    println!("Parsed JID: {}", jid);
    
    // Build text message using ExtendedTextMessage for better compatibility
    let wa_message = wa::Message {
        extended_text_message: Some(Box::new(wa::message::ExtendedTextMessage {
            text: Some(message.clone()),
            ..Default::default()
        })),
        ..Default::default()
    };

    println!("Attempting to send message: {}", message);
    
    match client.send_message(jid, wa_message).await {
        Ok(msg_id) => {
            println!("Message sent successfully with ID: {}", msg_id);
            Ok(msg_id)
        }
        Err(e) => {
            eprintln!("Failed to send message: {}", e);
            Err(format!("Failed to send message: {}", e))
        }
    }
}

// Tauri Command: Send message with media
#[tauri::command]
pub async fn send_media_message(
    contact: String,
    message_text: String,
    media_path: String,
    media_type: String, // "image", "video", "document"
    state: State<'_, Arc<WhatsAppState>>,
) -> Result<String, String> {
    // Check if bot is ready
    let is_ready = *state.is_ready.lock().await;
    if !is_ready {
        return Err("WhatsApp is not ready yet. Please wait for connection to complete.".to_string());
    }

    // Get client reference and release the bot lock immediately
    let client = {
        let bot_guard = state.bot.lock().await;
        let bot = bot_guard.as_ref().ok_or("WhatsApp not initialized")?;
        bot.client()
    };
    
    // Parse contact to JID format
    let clean_contact = contact.replace(['+', ' ', '-'], "");
    let jid = Jid::new(&clean_contact, "s.whatsapp.net");
    
    println!("Sending {} to: {}", media_type, clean_contact);
    
    // Read media file
    let media_data = std::fs::read(&media_path).map_err(|e| e.to_string())?;
    println!("Read media file: {} bytes", media_data.len());
    
    // Determine media type and MIME type
    let (media_type_enum, mime_type) = get_media_type_and_mime(&media_type, &media_path);
    
    // Upload media using the correct API
    println!("Uploading media...");
    let uploaded = client
        .upload(media_data, media_type_enum)
        .await
        .map_err(|e| {
            eprintln!("Upload failed: {}", e);
            e.to_string()
        })?;
    
    println!("Media uploaded successfully");
    
    // Build message with media based on type
    let wa_message = match media_type.as_str() {
        "image" => {
            let mut img_msg = wa::message::ImageMessage {
                url: Some(uploaded.url),
                direct_path: Some(uploaded.direct_path),
                media_key: Some(uploaded.media_key.to_vec()),
                file_enc_sha256: Some(uploaded.file_enc_sha256.to_vec()),
                file_sha256: Some(uploaded.file_sha256.to_vec()),
                file_length: Some(uploaded.file_length),
                mimetype: Some(mime_type),
                ..Default::default()
            };
            if !message_text.is_empty() {
                img_msg.caption = Some(message_text);
            }
            wa::Message {
                image_message: Some(Box::new(img_msg)),
                ..Default::default()
            }
        },
        "video" => {
            let mut vid_msg = wa::message::VideoMessage {
                url: Some(uploaded.url),
                direct_path: Some(uploaded.direct_path),
                media_key: Some(uploaded.media_key.to_vec()),
                file_enc_sha256: Some(uploaded.file_enc_sha256.to_vec()),
                file_sha256: Some(uploaded.file_sha256.to_vec()),
                file_length: Some(uploaded.file_length),
                mimetype: Some(mime_type),
                ..Default::default()
            };
            if !message_text.is_empty() {
                vid_msg.caption = Some(message_text);
            }
            wa::Message {
                video_message: Some(Box::new(vid_msg)),
                ..Default::default()
            }
        },
        "document" | _ => {
            let doc_msg = wa::message::DocumentMessage {
                url: Some(uploaded.url),
                direct_path: Some(uploaded.direct_path),
                media_key: Some(uploaded.media_key.to_vec()),
                file_enc_sha256: Some(uploaded.file_enc_sha256.to_vec()),
                file_sha256: Some(uploaded.file_sha256.to_vec()),
                file_length: Some(uploaded.file_length),
                mimetype: Some(mime_type),
                file_name: Some(
                    std::path::Path::new(&media_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("document")
                        .to_string()
                ),
                ..Default::default()
            };
            wa::Message {
                document_message: Some(Box::new(doc_msg)),
                ..Default::default()
            }
        },
    };
    
    match client.send_message(jid, wa_message).await {
        Ok(msg_id) => {
            println!("Media message sent successfully with ID: {}", msg_id);
            Ok(msg_id)
        }
        Err(e) => {
            eprintln!("Failed to send media message: {}", e);
            Err(format!("Failed to send media message: {}", e))
        }
    }
}

// Helper function to determine MediaType and MIME type
fn get_media_type_and_mime(type_str: &str, file_path: &str) -> (MediaType, String) {
    let extension = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    
    match type_str {
        "image" => {
            let mime = match extension.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "png" => "image/png",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "image/jpeg",
            };
            (MediaType::Image, mime.to_string())
        },
        "video" => {
            let mime = match extension.as_str() {
                "mp4" => "video/mp4",
                "mov" => "video/quicktime",
                "avi" => "video/x-msvideo",
                "mkv" => "video/x-matroska",
                _ => "video/mp4",
            };
            (MediaType::Video, mime.to_string())
        },
        "audio" => {
            let mime = match extension.as_str() {
                "mp3" => "audio/mpeg",
                "ogg" => "audio/ogg",
                "wav" => "audio/wav",
                "m4a" => "audio/mp4",
                _ => "audio/mpeg",
            };
            (MediaType::Audio, mime.to_string())
        },
        _ => {
            let mime = match extension.as_str() {
                "pdf" => "application/pdf",
                "doc" => "application/msword",
                "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                "xls" => "application/vnd.ms-excel",
                "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "zip" => "application/zip",
                "txt" => "text/plain",
                _ => "application/octet-stream",
            };
            (MediaType::Document, mime.to_string())
        }
    }
}