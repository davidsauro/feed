#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::stream::StreamExt;
use libp2p::{
    identity::Keypair, mdns, noise, swarm::{NetworkBehaviour, SwarmEvent}, tcp, yamux, PeerId,
    request_response::{cbor, Config as ReqResConfig, ProtocolSupport, Event as ReqResEvent, Message as ReqResMessage},
    StreamProtocol,
};
use rusqlite::{Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, sync::Mutex, time::Duration};
use tauri::{Emitter, Manager, State};
use tokio::sync::mpsc;

#[derive(Serialize, Deserialize)]
struct Contact {
    peer_id: String,
    nickname: String,
}

// NEW: Chat Message Struct for Database
#[derive(Serialize, Deserialize)]
struct ChatMessage {
    id: String,
    peer_id: String,
    sender: String,
    text: String,
    status: String,
}

#[derive(Clone, Serialize)]
struct ChatPayload {
    sender: String,
    message: String,
}

enum NetworkCommand {
    SendMessage { peer_id: String, message: String },
}

struct NetworkState {
    active_peers: Mutex<HashSet<String>>,
    network_tx: Mutex<mpsc::Sender<NetworkCommand>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChatRequest(String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChatResponse();

#[derive(NetworkBehaviour)]
struct AppBehaviour {
    mdns: mdns::tokio::Behaviour,
    chat: cbor::Behaviour<ChatRequest, ChatResponse>,
}

fn get_or_create_keypair(app_data_dir: &std::path::Path, instance: &str) -> Result<Keypair, String> {
    fs::create_dir_all(app_data_dir).map_err(|e| e.to_string())?;
    let key_path = app_data_dir.join(format!("identity_{}.bin", instance));
    if key_path.exists() {
        let bytes = fs::read(&key_path).map_err(|e| e.to_string())?;
        Keypair::from_protobuf_encoding(&bytes).map_err(|e| e.to_string())
    } else {
        let new_key = Keypair::generate_ed25519();
        let bytes = new_key.to_protobuf_encoding().map_err(|e| e.to_string())?;
        fs::write(&key_path, bytes).map_err(|e| e.to_string())?;
        Ok(new_key)
    }
}

fn get_db_connection(app: &tauri::AppHandle) -> SqlResult<Connection> {
    let app_data_dir = app.path().app_local_data_dir().unwrap();
    let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
    let db_path = app_data_dir.join(format!("contacts_{}.db", instance));
    Connection::open(db_path)
}

fn is_contact(app: &tauri::AppHandle, peer_id: &str) -> bool {
    if let Ok(conn) = get_db_connection(app) {
        let mut stmt = conn.prepare("SELECT COUNT(1) FROM contacts WHERE peer_id = ?1").unwrap();
        let count: i32 = stmt.query_row([peer_id], |row| row.get(0)).unwrap_or(0);
        return count> 0;
    }
    false
}

#[tauri::command]
fn get_node_id() -> String {
    std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string())
}

#[tauri::command]
fn get_identity(app: tauri::AppHandle) -> Result<String, String> {
    let app_data_dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
    let keypair = get_or_create_keypair(&app_data_dir, &instance)?;
    Ok(PeerId::from(keypair.public()).to_string())
}

#[tauri::command]
fn save_contact(app: tauri::AppHandle, peer_id: String, nickname: String) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO contacts (peer_id, nickname) VALUES (?1, ?2)",
        (&peer_id, &nickname),
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_contacts(app: tauri::AppHandle) -> Result<Vec<Contact>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT peer_id, nickname FROM contacts").map_err(|e| e.to_string())?;
    let contact_iter = stmt.query_map([], |row| {
        Ok(Contact { peer_id: row.get(0)?, nickname: row.get(1)? })
    }).map_err(|e| e.to_string())?;

    let mut contacts = Vec::new();
    for contact in contact_iter { contacts.push(contact.unwrap()); }
    Ok(contacts)
}

// NEW DB COMMANDS: Save, Update, and Fetch Chat History
#[tauri::command]
fn save_chat_message(app: tauri::AppHandle, id: String, peer_id: String, sender: String, text: String, status: String) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT OR REPLACE INTO messages (id, peer_id, sender, text, status) VALUES (?1, ?2, ?3, ?4, ?5)",
        (&id, &peer_id, &sender, &text, &status),
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn update_message_status(app: tauri::AppHandle, id: String, status: String) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    conn.execute("UPDATE messages SET status = ?1 WHERE id = ?2", (&status, &id)).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_chat_history(app: tauri::AppHandle, peer_id: String) -> Result<Vec<ChatMessage>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT id, peer_id, sender, text, status FROM messages WHERE peer_id = ?1 ORDER BY timestamp ASC").map_err(|e| e.to_string())?;
    let iter = stmt.query_map([&peer_id], |row| {
        Ok(ChatMessage {
            id: row.get(0)?,
            peer_id: row.get(1)?,
            sender: row.get(2)?,
            text: row.get(3)?,
            status: row.get(4)?,
        })
    }).map_err(|e| e.to_string())?;

    let mut msgs = Vec::new();
    for m in iter { msgs.push(m.unwrap()); }
    Ok(msgs)
}

#[tauri::command]
fn get_active_peers(state: State<'_, NetworkState>) -> Result<Vec<String>, String> {
    let peers = state.active_peers.lock().map_err(|e| e.to_string())?;
    Ok(peers.iter().cloned().collect())
}

#[tauri::command]
async fn send_message(state: State<'_, NetworkState>, peer_id: String, message: String) -> Result<(), String> {
    let tx = state.network_tx.lock().unwrap().clone();
    tx.send(NetworkCommand::SendMessage { peer_id, message }).await.map_err(|e| e.to_string())?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();
            let app_data_dir = app.path().app_local_data_dir().unwrap();
            let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
            
            let (network_tx, mut network_rx) = mpsc::channel(100);
            app_handle.manage(NetworkState { active_peers: Mutex::new(HashSet::new()), network_tx: Mutex::new(network_tx) });
            
            if let Ok(conn) = get_db_connection(&app_handle) {
                conn.execute("CREATE TABLE IF NOT EXISTS contacts (peer_id TEXT PRIMARY KEY, nickname TEXT NOT NULL)", ()).unwrap();
                // NEW: Create messages table
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS messages (
                        id TEXT PRIMARY KEY,
                        peer_id TEXT NOT NULL,
                        sender TEXT NOT NULL,
                        text TEXT NOT NULL,
                        status TEXT NOT NULL,
                        timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
                    )", ()
                ).unwrap();
            }
            
            let keypair = get_or_create_keypair(&app_data_dir, &instance).unwrap();
            
            tauri::async_runtime::spawn(async move {
                let req_res = cbor::Behaviour::<ChatRequest, ChatResponse>::new(
                    [(StreamProtocol::new("/chat/1.0.0"), ProtocolSupport::Full)],
                    ReqResConfig::default(),
                );

                let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
                    .with_tokio()
                    .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default).unwrap()
                    .with_behaviour(|key| {
                        AppBehaviour { mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id()).unwrap(), chat: req_res }
                    }).unwrap()
                    .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
                    .build();

                swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap()).unwrap();

                loop {
                    tokio::select! {
                        event = swarm.select_next_some() => {
                            match event {
                                SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                                    let state = app_handle.state::<NetworkState>();
                                    for (peer_id, _) in list {
                                        let peer_str = peer_id.to_string();
                                        if let Ok(mut peers) = state.active_peers.lock() { peers.insert(peer_str.clone()); }
                                        app_handle.emit("peer-discovered", peer_str).unwrap();
                                    }
                                }
                                SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                                    let state = app_handle.state::<NetworkState>();
                                    for (peer_id, _) in list {
                                        let peer_str = peer_id.to_string();
                                        if let Ok(mut peers) = state.active_peers.lock() { peers.remove(&peer_str); }
                                        app_handle.emit("peer-lost", peer_str).unwrap();
                                    }
                                }
                                SwarmEvent::Behaviour(AppBehaviourEvent::Chat(ReqResEvent::Message { peer, message, .. })) => {
                                    if let ReqResMessage::Request { request, channel, .. } = message {
                                        let sender_id = peer.to_string();
                                        if is_contact(&app_handle, &sender_id) {
                                            app_handle.emit("chat-received", ChatPayload { sender: sender_id, message: request.0 }).unwrap();
                                            let _ = swarm.behaviour_mut().chat.send_response(channel, ChatResponse());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Some(cmd) = network_rx.recv() => {
                            match cmd {
                                NetworkCommand::SendMessage { peer_id, message } => {
                                    if let Ok(peer) = peer_id.parse::<PeerId>() {
                                        swarm.behaviour_mut().chat.send_request(&peer, ChatRequest(message));
                                    }
                                }
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        // Register new commands
        .invoke_handler(tauri::generate_handler![
            get_identity, save_contact, get_contacts, get_active_peers, get_node_id, send_message,
            save_chat_message, update_message_status, get_chat_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}