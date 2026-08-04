#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use libp2p::identity::{Keypair, PeerId};
use std::fs;
use tauri::Manager;

// Notice this now returns Result because reading/writing files can fail
#[tauri::command]
fn get_identity(app: tauri::AppHandle) -> Result<String, String> {
    // Find the OS-specific local data folder for this app
    let app_data_dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    
    // Ensure the directory exists before we try to write to it
    fs::create_dir_all(&app_data_dir).map_err(|e| e.to_string())?;
    
    // The path to our secret key file
    let key_path = app_data_dir.join("identity.bin");

    let keypair = if key_path.exists() {
        // LOAD EXISTING: Read the bytes and decode them back into a Keypair
        let bytes = fs::read(&key_path).map_err(|e| e.to_string())?;
        Keypair::from_protobuf_encoding(&bytes).map_err(|e| e.to_string())?
    } else {
        // GENERATE NEW: Create a new Keypair, encode it to bytes, and save it
        let new_key = Keypair::generate_ed25519();
        let bytes = new_key.to_protobuf_encoding().map_err(|e| e.to_string())?;
        fs::write(&key_path, bytes).map_err(|e| e.to_string())?;
        new_key
    };
    
    // Derive the PeerId from whatever keypair we ended up with
    let local_peer_id = PeerId::from(keypair.public());
    
    Ok(format!("Your Node ID: {}", local_peer_id))
}

fn main() {
    tauri::Builder::default()
        // Make sure the command name matches what we use in Vue
        .invoke_handler(tauri::generate_handler![get_identity])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}