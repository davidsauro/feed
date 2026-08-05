// Prevent an extra console window from opening on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Entry point for the Rust side of the app.
//!
//! Three things live in here:
//!
//! 1. SQLite storage for contacts and chat history.
//! 2. Tauri commands, which are the functions the Vue frontend can call.
//! 3. A background libp2p task that discovers peers on the local network and
//!    moves chat messages between them.
//!
//! The frontend and the network task never talk to each other directly. The
//! frontend sends work to the network task through an mpsc channel, and the
//! network task sends news back to the frontend as Tauri events.

use futures::stream::StreamExt;
use libp2p::identity::Keypair;
use libp2p::request_response::cbor;
use libp2p::request_response::Config as RequestResponseConfig;
use libp2p::request_response::Event as RequestResponseEvent;
use libp2p::request_response::Message as RequestResponseMessage;
use libp2p::request_response::ProtocolSupport;
use libp2p::request_response::ResponseChannel;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::SwarmEvent;
use libp2p::{mdns, noise, tcp, yamux};
use libp2p::{PeerId, StreamProtocol, Swarm, SwarmBuilder};
use rusqlite::Connection;
use rusqlite::Result as SqlResult;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Used when the `NODE_ID` environment variable is not set. Running several
/// nodes on one machine means giving each one a different `NODE_ID` so they
/// don't share an identity file or a database.
const DEFAULT_NODE_ID: &str = "1";

/// Name that both sides of a connection must agree on before they can chat.
const CHAT_PROTOCOL: &str = "/chat/1.0.0";

/// Listen on every network interface, and let the OS pick the port.
const LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/tcp/0";

/// How many outbound commands the frontend can queue up before `send_message`
/// starts waiting for the network task to catch up.
const COMMAND_CHANNEL_CAPACITY: usize = 100;

/// How long a connection with no traffic on it is kept open.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

/// How often we ask the local network who's out there.
///
/// This is also how often a peer that's still alive refreshes its record, so it
/// has to be comfortably shorter than the TTL below or live peers would flicker
/// offline between queries.
const MDNS_QUERY_INTERVAL: Duration = Duration::from_secs(10);

/// How long a peer stays "online" without us hearing from it again.
///
/// This is what decides how quickly a node that went away is shown as offline. A
/// process that is killed has no chance to announce that it's leaving, so the
/// record expiring is the only signal we get. libp2p defaults to six minutes,
/// which leaves a dead node looking online for far too long.
const MDNS_RECORD_TTL: Duration = Duration::from_secs(30);

/// Reads the ID of this node from the environment.
///
/// Called from several places (identity file name, database file name, and the
/// indicator in the UI) and they all have to agree, so it lives in one spot.
fn current_node_id() -> String {
    std::env::var("NODE_ID").unwrap_or_else(|_| DEFAULT_NODE_ID.to_string())
}

// ---------------------------------------------------------------------------
// Types shared with the frontend
// ---------------------------------------------------------------------------

/// A peer the user has given a name to. Rows in the `contacts` table.
#[derive(Serialize, Deserialize)]
struct Contact {
    peer_id: String,
    nickname: String,
}

/// One chat message. Rows in the `messages` table.
///
/// `peer_id` is the other side of the conversation no matter who sent the
/// message, which is what lets us load a whole conversation with one query.
/// `sender` is who actually wrote it. `status` is one of "sending",
/// "delivered", or "read".
#[derive(Serialize, Deserialize)]
struct ChatMessage {
    id: String,
    peer_id: String,
    sender: String,
    text: String,
    status: String,
}

/// Emitted to the frontend as the `chat-received` event.
///
/// `message` is the raw string that came off the network. The frontend parses
/// it as JSON to tell a chat message apart from a read receipt.
#[derive(Clone, Serialize)]
struct ChatPayload {
    sender: String,
    message: String,
}

// ---------------------------------------------------------------------------
// Network plumbing
// ---------------------------------------------------------------------------

/// Work the frontend asks the background network task to do.
enum NetworkCommand {
    SendMessage { peer_id: String, message: String },
}

/// Shared state that Tauri hands to commands that ask for it.
struct NetworkState {
    /// Peers mDNS has seen recently and not yet reported as expired.
    active_peers: Mutex<HashSet<String>>,
    /// The sending half of the channel into the background network task.
    network_tx: Mutex<mpsc::Sender<NetworkCommand>>,
}

/// The bytes we send when we send a chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChatRequest(String);

/// The reply to a `ChatRequest`. Empty on purpose: receiving a reply at all is
/// the only information we need, which is that the other node accepted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChatResponse();

/// Everything this node does on the network.
///
/// The derive macro also generates an `AppBehaviourEvent` enum with one variant
/// per field below, which is what the swarm event loop matches on.
#[derive(NetworkBehaviour)]
struct AppBehaviour {
    /// Finds other nodes on the same local network.
    mdns: mdns::tokio::Behaviour,
    /// Sends and receives chat messages.
    chat: cbor::Behaviour<ChatRequest, ChatResponse>,
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Loads this node's keypair from disk, generating and saving one on first run.
///
/// The keypair is the node's identity: its PeerId is derived from the public
/// key, so keeping the same file means keeping the same address on the network
/// between restarts.
fn get_or_create_keypair(app_data_dir: &Path, node_id: &str) -> Result<Keypair, String> {
    fs::create_dir_all(app_data_dir).map_err(|e| e.to_string())?;

    let key_path = app_data_dir.join(format!("identity_{}.bin", node_id));

    if key_path.exists() {
        let bytes = fs::read(&key_path).map_err(|e| e.to_string())?;
        let keypair = Keypair::from_protobuf_encoding(&bytes).map_err(|e| e.to_string())?;
        return Ok(keypair);
    }

    let keypair = Keypair::generate_ed25519();
    let bytes = keypair.to_protobuf_encoding().map_err(|e| e.to_string())?;
    fs::write(&key_path, bytes).map_err(|e| e.to_string())?;

    Ok(keypair)
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// Opens this node's SQLite database, creating the file if it isn't there yet.
///
/// Connections are cheap and are not thread safe, so every caller opens its own
/// rather than sharing one.
fn get_db_connection(app: &AppHandle) -> SqlResult<Connection> {
    let app_data_dir = app
        .path()
        .app_local_data_dir()
        .expect("no local data directory for this app");

    let db_path = app_data_dir.join(format!("contacts_{}.db", current_node_id()));

    Connection::open(db_path)
}

/// Creates the tables the app needs, if they don't already exist.
///
/// Safe to call on every startup.
fn create_tables(conn: &Connection) -> SqlResult<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS contacts (
            peer_id  TEXT PRIMARY KEY,
            nickname TEXT NOT NULL
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS messages (
            id        TEXT PRIMARY KEY,
            peer_id   TEXT NOT NULL,
            sender    TEXT NOT NULL,
            text      TEXT NOT NULL,
            status    TEXT NOT NULL,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        (),
    )?;

    Ok(())
}

/// Answers whether we have saved this peer as a contact.
///
/// This is the spam filter for inbound messages, so anything unexpected (a
/// database that won't open, a query that fails) has to answer "no".
fn is_contact(app: &AppHandle, peer_id: &str) -> bool {
    let conn = match get_db_connection(app) {
        Ok(conn) => conn,
        Err(error) => {
            eprintln!("could not open database to check contact: {}", error);
            return false;
        }
    };

    let mut statement = match conn.prepare("SELECT COUNT(1) FROM contacts WHERE peer_id = ?1") {
        Ok(statement) => statement,
        Err(error) => {
            eprintln!("could not prepare contact lookup: {}", error);
            return false;
        }
    };

    let count: i64 = statement.query_row([peer_id], |row| row.get(0)).unwrap_or(0);

    count > 0
}

// ---------------------------------------------------------------------------
// Tauri commands: identity
// ---------------------------------------------------------------------------

/// Returns this node's ID, which the UI shows in the corner of the window.
#[tauri::command]
fn get_node_id() -> String {
    current_node_id()
}

/// Returns this node's PeerId, the address other nodes use to reach us.
#[tauri::command]
fn get_identity(app: AppHandle) -> Result<String, String> {
    let app_data_dir = app.path().app_local_data_dir().map_err(|e| e.to_string())?;
    let keypair = get_or_create_keypair(&app_data_dir, &current_node_id())?;
    let peer_id = PeerId::from(keypair.public());

    Ok(peer_id.to_string())
}

// ---------------------------------------------------------------------------
// Tauri commands: contacts
// ---------------------------------------------------------------------------

/// Adds a contact, or renames one we already have.
#[tauri::command]
fn save_contact(app: AppHandle, peer_id: String, nickname: String) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO contacts (peer_id, nickname) VALUES (?1, ?2)",
        (&peer_id, &nickname),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Removes a contact along with the whole conversation with them, and returns
/// how many messages were deleted.
///
/// Both deletes run in one transaction, so a failure leaves the contact and
/// their history intact rather than half-erasing one of them.
///
/// There is no undo. The frontend is responsible for confirming with the user
/// before calling this.
#[tauri::command]
fn delete_contact(app: AppHandle, peer_id: String) -> Result<usize, String> {
    let mut conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let transaction = conn.transaction().map_err(|e| e.to_string())?;

    let deleted_messages = transaction
        .execute("DELETE FROM messages WHERE peer_id = ?1", [&peer_id])
        .map_err(|e| e.to_string())?;

    transaction
        .execute("DELETE FROM contacts WHERE peer_id = ?1", [&peer_id])
        .map_err(|e| e.to_string())?;

    transaction.commit().map_err(|e| e.to_string())?;

    Ok(deleted_messages)
}

/// Returns how many messages are stored for one peer.
///
/// Used to tell the user what they're about to lose when removing a contact,
/// without loading every message to count them.
#[tauri::command]
fn count_chat_messages(app: AppHandle, peer_id: String) -> Result<usize, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    // SQLite counts are signed integers, so the count comes back as i64 and is
    // widened to usize for the frontend.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(1) FROM messages WHERE peer_id = ?1",
            [&peer_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    Ok(count.max(0) as usize)
}

/// Returns every saved contact.
#[tauri::command]
fn get_contacts(app: AppHandle) -> Result<Vec<Contact>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare("SELECT peer_id, nickname FROM contacts")
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map([], |row| {
            Ok(Contact {
                peer_id: row.get(0)?,
                nickname: row.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut contacts = Vec::new();
    for row in rows {
        let contact = row.map_err(|e| e.to_string())?;
        contacts.push(contact);
    }

    Ok(contacts)
}

// ---------------------------------------------------------------------------
// Tauri commands: chat history
// ---------------------------------------------------------------------------

/// Writes a message to the database, or overwrites it if the id already exists.
#[tauri::command]
fn save_chat_message(
    app: AppHandle,
    id: String,
    peer_id: String,
    sender: String,
    text: String,
    status: String,
) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR REPLACE INTO messages (id, peer_id, sender, text, status)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        (&id, &peer_id, &sender, &text, &status),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Moves a message along to its next status: sending, delivered, or read.
#[tauri::command]
fn update_message_status(app: AppHandle, id: String, status: String) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE messages SET status = ?1 WHERE id = ?2",
        (&status, &id),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Returns one conversation, oldest message first.
#[tauri::command]
fn get_chat_history(app: AppHandle, peer_id: String) -> Result<Vec<ChatMessage>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare(
            "SELECT id, peer_id, sender, text, status
             FROM messages
             WHERE peer_id = ?1
             ORDER BY timestamp ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map([&peer_id], |row| {
            Ok(ChatMessage {
                id: row.get(0)?,
                peer_id: row.get(1)?,
                sender: row.get(2)?,
                text: row.get(3)?,
                status: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut messages = Vec::new();
    for row in rows {
        let message = row.map_err(|e| e.to_string())?;
        messages.push(message);
    }

    Ok(messages)
}

// ---------------------------------------------------------------------------
// Tauri commands: network
// ---------------------------------------------------------------------------

/// Returns the peers currently visible on the local network.
///
/// The frontend also listens for `peer-discovered` and `peer-lost` events. This
/// command exists so a window that just opened can catch up on peers that were
/// discovered before it started listening.
#[tauri::command]
fn get_active_peers(state: State<'_, NetworkState>) -> Result<Vec<String>, String> {
    let active_peers = state.active_peers.lock().map_err(|e| e.to_string())?;

    Ok(active_peers.iter().cloned().collect())
}

/// Hands a message to the background network task to send.
///
/// This returns as soon as the command is queued, not when the message arrives.
#[tauri::command]
async fn send_message(
    state: State<'_, NetworkState>,
    peer_id: String,
    message: String,
) -> Result<(), String> {
    // Clone the sender and drop the lock right away. Holding a std::sync::Mutex
    // across the await below would block the async runtime.
    let network_tx = {
        let guard = state.network_tx.lock().map_err(|e| e.to_string())?;
        guard.clone()
    };

    let command = NetworkCommand::SendMessage { peer_id, message };
    network_tx.send(command).await.map_err(|e| e.to_string())?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Background network task
// ---------------------------------------------------------------------------

/// Builds the libp2p swarm: our identity plus the transport and behaviours it
/// speaks over.
///
/// Panics on failure, because a node that can't set up its network stack has
/// nothing useful to do.
fn build_swarm(keypair: Keypair) -> Swarm<AppBehaviour> {
    let chat = cbor::Behaviour::<ChatRequest, ChatResponse>::new(
        [(
            StreamProtocol::new(CHAT_PROTOCOL),
            ProtocolSupport::Full,
        )],
        RequestResponseConfig::default(),
    );

    SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .expect("failed to configure the TCP transport")
        .with_behaviour(|key| {
            let mdns_config = mdns::Config {
                ttl: MDNS_RECORD_TTL,
                query_interval: MDNS_QUERY_INTERVAL,
                ..mdns::Config::default()
            };

            let mdns = mdns::tokio::Behaviour::new(mdns_config, key.public().to_peer_id())
                .expect("failed to start mDNS discovery");

            AppBehaviour { mdns, chat }
        })
        .expect("failed to configure the network behaviours")
        .with_swarm_config(|config| {
            config.with_idle_connection_timeout(IDLE_CONNECTION_TIMEOUT)
        })
        .build()
}

/// Sends an event to the frontend, logging instead of panicking if it fails.
///
/// Called from the network loop, which has to keep running even when the window
/// is gone.
fn emit_to_frontend<P: Serialize + Clone>(app: &AppHandle, event: &str, payload: P) {
    if let Err(error) = app.emit(event, payload) {
        eprintln!("could not emit '{}' to the frontend: {}", event, error);
    }
}

/// Records newly discovered peers and tells the frontend about them.
fn handle_peers_discovered(app: &AppHandle, peers: Vec<PeerId>) {
    let state = app.state::<NetworkState>();

    for peer_id in peers {
        let peer_id = peer_id.to_string();

        match state.active_peers.lock() {
            Ok(mut active_peers) => {
                active_peers.insert(peer_id.clone());
            }
            Err(error) => eprintln!("could not record discovered peer: {}", error),
        }

        emit_to_frontend(app, "peer-discovered", peer_id);
    }
}

/// Forgets peers we haven't heard from and tells the frontend they went away.
fn handle_peers_expired(app: &AppHandle, peers: Vec<PeerId>) {
    let state = app.state::<NetworkState>();

    for peer_id in peers {
        let peer_id = peer_id.to_string();

        match state.active_peers.lock() {
            Ok(mut active_peers) => {
                active_peers.remove(&peer_id);
            }
            Err(error) => eprintln!("could not forget expired peer: {}", error),
        }

        emit_to_frontend(app, "peer-lost", peer_id);
    }
}

/// Handles one inbound chat message.
///
/// Messages from peers that are not in our contacts are dropped without a
/// reply, which keeps strangers on the local network from reaching the UI at
/// all.
fn handle_chat_request(
    app: &AppHandle,
    swarm: &mut Swarm<AppBehaviour>,
    sender: PeerId,
    request: ChatRequest,
    response_channel: ResponseChannel<ChatResponse>,
) {
    let sender_id = sender.to_string();

    if !is_contact(app, &sender_id) {
        return;
    }

    let ChatRequest(message) = request;

    emit_to_frontend(
        app,
        "chat-received",
        ChatPayload {
            sender: sender_id,
            message,
        },
    );

    // The reply is the sender's confirmation that we accepted the message. It
    // fails only if they disconnected while we were working, so ignore it.
    let _ = swarm
        .behaviour_mut()
        .chat
        .send_response(response_channel, ChatResponse());
}

/// Routes one event out of the swarm to the code that cares about it.
///
/// Events we don't act on (listening addresses, connections opening and
/// closing, and so on) fall through the catch-all arm.
fn handle_swarm_event(
    app: &AppHandle,
    swarm: &mut Swarm<AppBehaviour>,
    event: SwarmEvent<AppBehaviourEvent>,
) {
    match event {
        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Discovered(discovered))) => {
            // mDNS reports a (peer, address) pair per address; we only need the
            // peer, and the same peer can appear more than once.
            let peers = discovered.into_iter().map(|(peer_id, _address)| peer_id);
            handle_peers_discovered(app, peers.collect());
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Expired(expired))) => {
            let peers = expired.into_iter().map(|(peer_id, _address)| peer_id);
            handle_peers_expired(app, peers.collect());
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Chat(RequestResponseEvent::Message {
            peer,
            message,
            ..
        })) => {
            // We only ever act on requests. Responses are empty by design, and
            // arriving at all is all they tell us.
            if let RequestResponseMessage::Request {
                request, channel, ..
            } = message
            {
                handle_chat_request(app, swarm, peer, request, channel);
            }
        }

        _ => {}
    }
}

/// Carries out one command the frontend queued up.
fn handle_network_command(swarm: &mut Swarm<AppBehaviour>, command: NetworkCommand) {
    match command {
        NetworkCommand::SendMessage { peer_id, message } => {
            let peer_id = match peer_id.parse::<PeerId>() {
                Ok(peer_id) => peer_id,
                Err(error) => {
                    eprintln!("cannot send to '{}': {}", peer_id, error);
                    return;
                }
            };

            swarm
                .behaviour_mut()
                .chat
                .send_request(&peer_id, ChatRequest(message));
        }
    }
}

/// Runs the network for as long as the app is open.
///
/// Two things can wake this loop up: the swarm has news for us, or the frontend
/// has queued a command. `tokio::select!` waits on both and handles whichever
/// arrives first.
async fn run_network(
    app: AppHandle,
    keypair: Keypair,
    mut command_rx: mpsc::Receiver<NetworkCommand>,
) {
    let mut swarm = build_swarm(keypair);

    let listen_address = LISTEN_ADDRESS
        .parse()
        .expect("LISTEN_ADDRESS is not a valid multiaddr");

    swarm
        .listen_on(listen_address)
        .expect("failed to start listening for connections");

    loop {
        tokio::select! {
            swarm_event = swarm.select_next_some() => {
                handle_swarm_event(&app, &mut swarm, swarm_event);
            }

            Some(command) = command_rx.recv() => {
                handle_network_command(&mut swarm, command);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();

            // The channel the frontend uses to reach the network task. The
            // sending half goes into Tauri state so commands can find it; the
            // receiving half goes to the network task.
            let (network_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);

            app_handle.manage(NetworkState {
                active_peers: Mutex::new(HashSet::new()),
                network_tx: Mutex::new(network_tx),
            });

            // Make sure the database is usable before anything reads from it.
            match get_db_connection(&app_handle) {
                Ok(conn) => {
                    create_tables(&conn).expect("failed to create the database tables");
                }
                Err(error) => eprintln!("could not open the database: {}", error),
            }

            let app_data_dir = app
                .path()
                .app_local_data_dir()
                .expect("no local data directory for this app");

            let keypair = get_or_create_keypair(&app_data_dir, &current_node_id())
                .expect("failed to load or create this node's identity");

            tauri::async_runtime::spawn(run_network(app_handle, keypair, command_rx));

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Identity
            get_node_id,
            get_identity,
            // Contacts
            save_contact,
            get_contacts,
            delete_contact,
            // Chat history
            save_chat_message,
            update_message_status,
            get_chat_history,
            count_chat_messages,
            // Network
            get_active_peers,
            send_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
