#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::stream::StreamExt;
use libp2p::{
    identity::Keypair, mdns, noise, swarm::{NetworkBehaviour, SwarmEvent}, tcp, yamux, PeerId,
};
use rusqlite::{Connection, Result as SqlResult};
use serde::{Deserialize, Serialize};
use std::{fs, time::Duration};
use tauri::{Emitter, Manager};

// A struct to represent a saved contact, deriving Serialize to send it to Vue
#[derive(Serialize, Deserialize)]
struct Contact {
    peer_id: String,
    nickname: String,
}

#[derive(NetworkBehaviour)]
struct AppBehaviour {
    mdns: mdns::tokio::Behaviour,
}

// --- Helper Functions ---

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

// Connects to the SQLite DB (creates it if it doesn't exist)
fn get_db_connection(app: &tauri::AppHandle) -> SqlResult<Connection> {
    let app_data_dir = app.path().app_local_data_dir().unwrap();
    let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
    let db_path = app_data_dir.join(format!("contacts_{}.db", instance));
    
    Connection::open(db_path)
}

// --- Tauri Commands (Callable from Vue) ---

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
    // Insert or replace ensures we can update nicknames for existing IDs
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
        Ok(Contact {
            peer_id: row.get(0)?,
            nickname: row.get(1)?,
        })
    }).map_err(|e| e.to_string())?;

    let mut contacts = Vec::new();
    for contact in contact_iter {
        contacts.push(contact.unwrap());
    }
    Ok(contacts)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();
            let app_data_dir = app.path().app_local_data_dir().unwrap();
            let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
            
            // Initialize the SQLite Database Table
            if let Ok(conn) = get_db_connection(&app_handle) {
                conn.execute(
                    "CREATE TABLE IF NOT EXISTS contacts (
                        peer_id TEXT PRIMARY KEY,
                        nickname TEXT NOT NULL
                    )",
                    (),
                ).expect("Failed to create database table");
            }
            
            let keypair = get_or_create_keypair(&app_data_dir, &instance).unwrap();
            
            tauri::async_runtime::spawn(async move {
                let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
                    .with_tokio()
                    .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default).unwrap()
                    .with_behaviour(|key| {
                        AppBehaviour { mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id()).unwrap() }
                    }).unwrap()
                    .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
                    .build();

                swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap()).unwrap();

                loop {
                    match swarm.select_next_some().await {
                        // When we discover a peer, EMIT it to the Vue frontend!
                        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                            for (peer_id, _) in list {
                                app_handle.emit("peer-discovered", peer_id.to_string()).unwrap();
                            }
                        }
                        // When they disconnect, emit an event so Vue can remove them
                        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                            for (peer_id, _) in list {
                                app_handle.emit("peer-lost", peer_id.to_string()).unwrap();
                            }
                        }
                        _ => {}
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_identity, save_contact, get_contacts])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}