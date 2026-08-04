#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures::stream::StreamExt;
use libp2p::{
    identity::Keypair, mdns, noise, swarm::{NetworkBehaviour, SwarmEvent}, tcp, yamux, PeerId
};
use std::{fs, time::Duration};
use tauri::Manager;

// Define our network protocols. For now, it's just mDNS.
#[derive(NetworkBehaviour)]
struct AppBehaviour {
    mdns: mdns::tokio::Behaviour,
}

// Helper function to handle the persistent identity logic
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

// Our existing UI command, refactored to use the helper
#[tauri::command]
fn get_identity(app: tauri::AppHandle) -> Result<String, String> {
    let app_data_dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
    
    let keypair = get_or_create_keypair(&app_data_dir, &instance)?;
    Ok(format!("Your Node ID: {}", PeerId::from(keypair.public())))
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app.path().app_local_data_dir().unwrap();
            let instance = std::env::var("NODE_ID").unwrap_or_else(|_| "1".to_string());
            
            // 1. Get our persistent identity for the network
            let keypair = get_or_create_keypair(&app_data_dir, &instance).expect("Failed to load keys");
            let local_peer_id = PeerId::from(keypair.public());
            
            // Spawn the network logic in a background Tokio thread so it doesn't block the UI
            tauri::async_runtime::spawn(async move {
                // 2. Build the Swarm (Identity + TCP + Encryption + mDNS)
                let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
                    .with_tokio()
                    .with_tcp(
                        tcp::Config::default(),
                        noise::Config::new,
                        yamux::Config::default,
                    ).expect("Failed to create TCP transport")
                    .with_behaviour(|key| {
                        AppBehaviour {
                            mdns: mdns::tokio::Behaviour::new(mdns::Config::default(), key.public().to_peer_id()).unwrap(),
                        }
                    }).expect("Failed to create behaviour")
                    .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
                    .build();

                // 3. Tell the OS to assign us any available port to listen on
                swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse().unwrap()).unwrap();
                println!("Node {} started! PeerID: {}", instance, local_peer_id);

                // 4. The main event loop
                loop {
                    match swarm.select_next_some().await {
                        SwarmEvent::NewListenAddr { address, .. } => {
                            println!("Local node is listening on {}", address);
                        }
                        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                            for (peer_id, multiaddr) in list {
                                println!("🟢 DISCOVERED PEER: {} at {}", peer_id, multiaddr);
                            }
                        }
                        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
                            for (peer_id, multiaddr) in list {
                                println!("🔴 PEER LOST: {} at {}", peer_id, multiaddr);
                            }
                        }
                        _ => {}
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![get_identity])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}