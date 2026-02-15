use std::sync::Arc;
use tauri::{Emitter, State, Window, Manager};
use tokio::sync::{Mutex, mpsc, oneshot};
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

// Commands sent to the bot task to avoid cross-thread Rc issues
enum BotCommand {
    SendMessage {
        jid: Jid,
        message: wa::Message,
        reply: oneshot::Sender<Result<String, String>>,
    },
    SendMediaMessage {
        jid: Jid,
        media_data: Vec<u8>,
        media_type_enum: MediaType,
        media_category: String,
        mime_type: String,
        caption: String,
        file_name: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
}

pub struct WhatsAppState {
    command_tx: Arc<Mutex<Option<mpsc::Sender<BotCommand>>>>,
    is_authenticated: Arc<Mutex<bool>>,
    is_ready: Arc<Mutex<bool>>,
}

impl WhatsAppState {
    pub fn new() -> Self {
        Self {
            command_tx: Arc::new(Mutex::new(None)),
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

    let (tx, mut rx) = mpsc::channel::<BotCommand>(32);
    *state.command_tx.lock().await = Some(tx);

    let window_clone = window.clone();
    let state_clone = state.inner().clone();
    
    tokio::spawn(async move {
        let state_for_events = state_clone.clone();
        let window_for_logout = window_clone.clone();
        
        let bot_result = Bot::builder()
            .with_backend(Arc::new(backend))
            .with_transport_factory(TokioWebSocketTransportFactory::new())
            .with_http_client(UreqHttpClient::new())
            .on_event(move |event, _client| {
                let window = window_clone.clone();
                let state = state_for_events.clone();
                
                async move {
                    match event {
                        Event::PairingQrCode { code, .. } => {
                            println!("QR Code generated");
                            let _ = window.emit("qr-code", QrCodeEvent { code });
                        }
                        
                        Event::PairSuccess(_) => {
                            println!("Pair success event received");
                            *state.is_authenticated.lock().await = true;
                            let _ = window.emit("auth-success", ());
                        }
                        
                        Event::Connected(_) => {
                            println!("Connected event received - Bot is fully ready");
                            *state.is_authenticated.lock().await = true;
                            *state.is_ready.lock().await = true;
                            let _ = window.emit("auth-success", ());
                        }
                        
                        Event::LoggedOut(_) => {
                            println!("Logged out event received");
                            *state.is_authenticated.lock().await = false;
                            *state.is_ready.lock().await = false;
                            let _ = window.emit("logged-out", ());
                        }
                        
                        Event::Message(_msg, info) => {
                            println!("Message received from: {:?}", info.source.sender);
                        }
                        
                        _ => {}
                    }
                }
            })
            .build()
            .await;

        match bot_result {
            Ok(mut bot) => {
                println!("Bot built successfully, starting...");
                match bot.run().await {
                    Ok(handle) => {
                        println!("Bot started successfully");
                        let client = bot.client();
                        
                        // Process commands via channel on the SAME task as the bot.
                        // This avoids cross-thread Rc access that causes crashes.
                        tokio::pin!(handle);
                        loop {
                            tokio::select! {
                                cmd = rx.recv() => {
                                    match cmd {
                                        Some(BotCommand::SendMessage { jid, message, reply }) => {
                                            println!("Processing SendMessage command");
                                            let result = client.send_message(jid, message).await
                                                .map_err(|e| format!("Failed to send: {}", e));
                                            let _ = reply.send(result);
                                        }
                                        Some(BotCommand::SendMediaMessage {
                                            jid, media_data, media_type_enum,
                                            media_category, mime_type, caption,
                                            file_name, reply
                                        }) => {
                                            println!("Processing SendMediaMessage command");
                                            let result = async {
                                                println!("Uploading media...");
                                                let uploaded = client.upload(media_data, media_type_enum)
                                                    .await.map_err(|e| {
                                                        eprintln!("Upload failed: {}", e);
                                                        e.to_string()
                                                    })?;
                                                println!("Media uploaded successfully");
                                                
                                                let wa_message = match media_category.as_str() {
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
                                                        if !caption.is_empty() {
                                                            img_msg.caption = Some(caption);
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
                                                        if !caption.is_empty() {
                                                            vid_msg.caption = Some(caption);
                                                        }
                                                        wa::Message {
                                                            video_message: Some(Box::new(vid_msg)),
                                                            ..Default::default()
                                                        }
                                                    },
                                                    _ => {
                                                        let doc_msg = wa::message::DocumentMessage {
                                                            url: Some(uploaded.url),
                                                            direct_path: Some(uploaded.direct_path),
                                                            media_key: Some(uploaded.media_key.to_vec()),
                                                            file_enc_sha256: Some(uploaded.file_enc_sha256.to_vec()),
                                                            file_sha256: Some(uploaded.file_sha256.to_vec()),
                                                            file_length: Some(uploaded.file_length),
                                                            mimetype: Some(mime_type),
                                                            file_name: Some(file_name),
                                                            ..Default::default()
                                                        };
                                                        wa::Message {
                                                            document_message: Some(Box::new(doc_msg)),
                                                            ..Default::default()
                                                        }
                                                    },
                                                };
                                                
                                                client.send_message(jid, wa_message).await
                                                    .map_err(|e| format!("Failed to send media: {}", e))
                                            }.await;
                                            let _ = reply.send(result);
                                        }
                                        None => {
                                            println!("Command channel closed");
                                            break;
                                        }
                                    }
                                }
                                _ = &mut handle => {
                                    println!("Bot handle completed");
                                    break;
                                }
                            }
                        }
                        
                        // Bot stopped - reset state
                        println!("Bot task ending, resetting state");
                        *state_clone.is_ready.lock().await = false;
                        *state_clone.is_authenticated.lock().await = false;
                        let _ = window_for_logout.emit("logged-out", ());
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
    let is_ready = *state.is_ready.lock().await;
    if !is_ready {
        return Err("WhatsApp is not ready yet. Please wait for connection to complete.".to_string());
    }

    let clean_contact = contact.replace(['+', ' ', '-'], "");
    println!("Sending message to contact: {}", clean_contact);
    
    let jid = Jid::new(&clean_contact, "s.whatsapp.net");
    println!("Parsed JID: {}", jid);
    
    let wa_message = wa::Message {
        extended_text_message: Some(Box::new(wa::message::ExtendedTextMessage {
            text: Some(message.clone()),
            ..Default::default()
        })),
        ..Default::default()
    };

    println!("Attempting to send message: {}", message);
    
    // Send command to bot task via channel (avoids cross-thread Rc crash)
    let (reply_tx, reply_rx) = oneshot::channel();
    
    let tx = {
        let guard = state.command_tx.lock().await;
        guard.as_ref().ok_or("WhatsApp not initialized")?.clone()
    };
    
    tx.send(BotCommand::SendMessage {
        jid,
        message: wa_message,
        reply: reply_tx,
    }).await.map_err(|_| "Failed to send command to bot task".to_string())?;
    
    match reply_rx.await {
        Ok(Ok(msg_id)) => {
            println!("Message sent successfully with ID: {}", msg_id);
            Ok(msg_id)
        }
        Ok(Err(e)) => {
            eprintln!("Failed to send message: {}", e);
            Err(e)
        }
        Err(_) => Err("Bot task dropped before responding".to_string()),
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
    let is_ready = *state.is_ready.lock().await;
    if !is_ready {
        return Err("WhatsApp is not ready yet. Please wait for connection to complete.".to_string());
    }

    let clean_contact = contact.replace(['+', ' ', '-'], "");
    let jid = Jid::new(&clean_contact, "s.whatsapp.net");
    
    println!("Sending {} to: {}", media_type, clean_contact);
    
    let media_data = std::fs::read(&media_path).map_err(|e| e.to_string())?;
    println!("Read media file: {} bytes", media_data.len());
    
    let (media_type_enum, mime_type) = get_media_type_and_mime(&media_type, &media_path);
    
    let file_name = std::path::Path::new(&media_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document")
        .to_string();
    
    // Send command to bot task via channel (avoids cross-thread Rc crash)
    let (reply_tx, reply_rx) = oneshot::channel();
    
    let tx = {
        let guard = state.command_tx.lock().await;
        guard.as_ref().ok_or("WhatsApp not initialized")?.clone()
    };
    
    tx.send(BotCommand::SendMediaMessage {
        jid,
        media_data,
        media_type_enum,
        media_category: media_type,
        mime_type,
        caption: message_text,
        file_name,
        reply: reply_tx,
    }).await.map_err(|_| "Failed to send command to bot task".to_string())?;
    
    match reply_rx.await {
        Ok(Ok(msg_id)) => {
            println!("Media message sent successfully with ID: {}", msg_id);
            Ok(msg_id)
        }
        Ok(Err(e)) => {
            eprintln!("Failed to send media message: {}", e);
            Err(e)
        }
        Err(_) => Err("Bot task dropped before responding".to_string()),
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