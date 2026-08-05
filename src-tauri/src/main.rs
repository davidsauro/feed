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
use libp2p::gossipsub;
use libp2p::identity::Keypair;
use libp2p::ping;
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
use tokio::sync::{mpsc, oneshot};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Used when the `NODE_ID` environment variable is not set. Running several
/// nodes on one machine means giving each one a different `NODE_ID` so they
/// don't share an identity file or a database.
const DEFAULT_NODE_ID: &str = "1";

/// Name that both sides of a connection must agree on before they can chat.
const CHAT_PROTOCOL: &str = "/chat/1.0.0";

/// Prefix for the gossipsub topic a group's messages travel on.
///
/// The topic is this prefix plus the group's id, so knowing the id is what lets
/// a node join the conversation.
const GROUP_TOPIC_PREFIX: &str = "/group/1.0.0/";

/// How often gossipsub maintains its mesh. The default is one second, which is
/// more upkeep than a handful of nodes on a LAN needs.
const GOSSIPSUB_HEARTBEAT: Duration = Duration::from_secs(10);

/// Listen on every network interface, and let the OS pick the port.
const LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/tcp/0";

/// How many outbound commands the frontend can queue up before `send_message`
/// starts waiting for the network task to catch up.
const COMMAND_CHANNEL_CAPACITY: usize = 100;

/// How long a connection with no traffic on it is kept open.
///
/// Long, because presence is decided by whether a connection exists: dropping an
/// idle connection between two live nodes would show them as having gone away. A
/// node that actually goes away closes its sockets, which is noticed instantly.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How often we ask the local network who's out there.
///
/// Shorter than libp2p's five minute default so a node that just started is
/// found quickly. This costs one small multicast packet per interval.
const MDNS_QUERY_INTERVAL: Duration = Duration::from_secs(10);

/// How long our mDNS record stays valid in other nodes' tables.
///
/// Kept generous on purpose. This governs how long other nodes retain our
/// *address*, which is what they need to dial us; it deliberately has nothing to
/// do with whether we're shown as online. See the note on presence above
/// `handle_peer_connected`.
const MDNS_RECORD_TTL: Duration = Duration::from_secs(6 * 60);

/// How often gossipsub retries connecting to peers it has been told about,
/// measured in heartbeats. Three heartbeats is 30 seconds, so a peer we failed
/// to reach — or lost a connection to — is picked up again promptly.
const GOSSIPSUB_EXPLICIT_PEER_TICKS: u64 = 3;

/// How often each connection is pinged, and how long a ping may go unanswered.
///
/// Together these bound how quickly a node that vanished without closing its
/// sockets — a machine suspended, unplugged, or losing Wi-Fi — is noticed. The
/// worst case is one interval plus one timeout, so 20 seconds: the peer dies
/// just after answering a ping, the next ping goes out an interval later, and it
/// takes a timeout to give up on it.
const PING_INTERVAL: Duration = Duration::from_secs(10);
const PING_TIMEOUT: Duration = Duration::from_secs(10);

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

/// A group conversation. Rows in the `groups` table, with members joined in.
///
/// Whoever creates a group decides who's in it. The member list travels with
/// every message so the others stay in step without a separate sync protocol.
#[derive(Serialize, Deserialize)]
struct Group {
    id: String,
    name: String,
    members: Vec<String>,
}

/// One chat message, direct or group. Rows in the `messages` table.
///
/// Exactly one of `peer_id` and `group_id` identifies the conversation. For a
/// direct message `peer_id` is the other side (whoever sent it) and `group_id`
/// is null; for a group message `group_id` is the group and `peer_id` is empty.
/// `sender` is always who actually wrote it, which is what lets a group message
/// be attributed.
///
/// `status` is one of "sending", "delivered", "read", or "failed". Group
/// messages stop at "delivered": gossipsub tells us a message reached the mesh,
/// not who read it.
#[derive(Serialize, Deserialize)]
struct ChatMessage {
    id: String,
    peer_id: String,
    group_id: Option<String>,
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

/// Emitted to the frontend as the `group-message-received` event.
///
/// Same shape as `ChatPayload` plus the group it belongs to. `sender` is taken
/// from the message signature rather than the payload, so it can't be forged by
/// whoever relayed it.
#[derive(Clone, Serialize)]
struct GroupPayload {
    group_id: String,
    sender: String,
    message: String,
}

// ---------------------------------------------------------------------------
// Network plumbing
// ---------------------------------------------------------------------------

/// Work the frontend asks the background network task to do.
///
/// Everything here needs the swarm, which only the network task can touch.
enum NetworkCommand {
    SendMessage {
        peer_id: String,
        message: String,
    },
    /// Start receiving a group's messages. Gossipsub only delivers to
    /// subscribers, so this has to happen before anything arrives.
    SubscribeToGroup {
        group_id: String,
    },
    /// Stop receiving a group's messages, on leaving it.
    UnsubscribeFromGroup {
        group_id: String,
    },
    /// Publish to a group.
    ///
    /// Unlike a direct message this reports back, because gossipsub refuses to
    /// publish when nobody is subscribed and the sender needs to know.
    PublishToGroup {
        group_id: String,
        message: String,
        result_tx: oneshot::Sender<Result<(), String>>,
    },
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
    /// Sends and receives direct chat messages, and group invites.
    chat: cbor::Behaviour<ChatRequest, ChatResponse>,
    /// Carries group messages, one topic per group.
    ///
    /// Unlike `chat`, this reaches members we have no direct connection to:
    /// peers in the middle relay what they receive.
    groups: gossipsub::Behaviour,
    /// Checks that the peer on the other end of each connection is still there.
    ///
    /// Only needed for nodes that disappear without closing their sockets; one
    /// that exits normally is noticed the moment the connection drops.
    ping: ping::Behaviour,
}

/// The gossipsub topic a group's messages travel on.
fn group_topic(group_id: &str) -> gossipsub::IdentTopic {
    gossipsub::IdentTopic::new(format!("{}{}", GROUP_TOPIC_PREFIX, group_id))
}

/// Recovers a group id from the topic a message arrived on.
///
/// Returns None for any topic that isn't one of ours.
fn group_id_from_topic(topic: &str) -> Option<&str> {
    topic.strip_prefix(GROUP_TOPIC_PREFIX)
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
            group_id  TEXT,
            sender    TEXT NOT NULL,
            text      TEXT NOT NULL,
            status    TEXT NOT NULL,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS groups (
            id         TEXT PRIMARY KEY,
            name       TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS group_members (
            group_id TEXT NOT NULL,
            peer_id  TEXT NOT NULL,
            PRIMARY KEY (group_id, peer_id)
        )",
        (),
    )?;

    add_missing_columns(conn)?;

    Ok(())
}

/// Brings a database created by an older version of the app up to date.
///
/// `CREATE TABLE IF NOT EXISTS` does nothing to a table that already exists, so
/// a column added later has to be applied by hand to databases already on disk.
/// Checked rather than attempted-and-ignored, because SQLite gives no way to say
/// "add this column only if it's missing".
fn add_missing_columns(conn: &Connection) -> SqlResult<()> {
    let has_group_id = {
        let mut statement = conn.prepare("PRAGMA table_info(messages)")?;

        // Column 1 of table_info is the column name.
        let names = statement.query_map([], |row| row.get::<_, String>(1))?;

        let mut found = false;
        for name in names {
            if name? == "group_id" {
                found = true;
            }
        }
        found
    };

    if !has_group_id {
        conn.execute("ALTER TABLE messages ADD COLUMN group_id TEXT", ())?;
    }

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
        .execute(
            "DELETE FROM messages WHERE peer_id = ?1 AND group_id IS NULL",
            [&peer_id],
        )
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
            "SELECT COUNT(1) FROM messages WHERE peer_id = ?1 AND group_id IS NULL",
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

/// Writes a message to the database, keeping what's already there on a clash.
///
/// `IGNORE` rather than `REPLACE` on purpose, and it matters. Message ids are
/// chosen by whoever sent the message, and we hand our own ids to the other side
/// in the payload so they can be acknowledged — so a contact knows the ids of
/// messages we sent them. With `REPLACE` they could send a message reusing one of
/// those ids and overwrite that row, rewriting our copy of the conversation:
/// putting words in our mouth, or erasing something they said.
///
/// Nothing legitimate ever needs to overwrite a stored message. A message is
/// written once, and later status changes go through `update_message_status`,
/// which can only move a row we already have.
///
/// Returns whether the message was stored. False means one with that id is
/// already on record, which the caller should treat as "not a new message".
#[tauri::command]
fn save_chat_message(
    app: AppHandle,
    id: String,
    peer_id: String,
    sender: String,
    text: String,
    status: String,
) -> Result<bool, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let rows_written = conn
        .execute(
            "INSERT OR IGNORE INTO messages (id, peer_id, sender, text, status)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (&id, &peer_id, &sender, &text, &status),
        )
        .map_err(|e| e.to_string())?;

    Ok(rows_written > 0)
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
            "SELECT id, peer_id, group_id, sender, text, status
             FROM messages
             WHERE peer_id = ?1 AND group_id IS NULL
             ORDER BY timestamp ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map([&peer_id], read_chat_message)
        .map_err(|e| e.to_string())?;

    collect_chat_messages(rows)
}

/// Builds a `ChatMessage` from a row selecting the columns in the order used by
/// `get_chat_history` and `get_group_history`.
fn read_chat_message(row: &rusqlite::Row) -> SqlResult<ChatMessage> {
    Ok(ChatMessage {
        id: row.get(0)?,
        peer_id: row.get(1)?,
        group_id: row.get(2)?,
        sender: row.get(3)?,
        text: row.get(4)?,
        status: row.get(5)?,
    })
}

/// Drains a query into a Vec, turning any row error into a message for the
/// frontend.
fn collect_chat_messages<I>(rows: I) -> Result<Vec<ChatMessage>, String>
where
    I: Iterator<Item = SqlResult<ChatMessage>>,
{
    let mut messages = Vec::new();

    for row in rows {
        let message = row.map_err(|e| e.to_string())?;
        messages.push(message);
    }

    Ok(messages)
}

// ---------------------------------------------------------------------------
// Tauri commands: groups
// ---------------------------------------------------------------------------

/// Creates a group, or updates one we already know about.
///
/// Called both when the user makes a group and when we learn about one from an
/// invite or a message, so the member list is replaced wholesale rather than
/// merged: whoever created the group decides who's in it.
#[tauri::command]
fn save_group(
    app: AppHandle,
    id: String,
    name: String,
    members: Vec<String>,
) -> Result<(), String> {
    let mut conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    let transaction = conn.transaction().map_err(|e| e.to_string())?;

    transaction
        .execute(
            "INSERT OR REPLACE INTO groups (id, name) VALUES (?1, ?2)",
            (&id, &name),
        )
        .map_err(|e| e.to_string())?;

    transaction
        .execute("DELETE FROM group_members WHERE group_id = ?1", [&id])
        .map_err(|e| e.to_string())?;

    for member in &members {
        transaction
            .execute(
                "INSERT OR REPLACE INTO group_members (group_id, peer_id) VALUES (?1, ?2)",
                (&id, member),
            )
            .map_err(|e| e.to_string())?;
    }

    transaction.commit().map_err(|e| e.to_string())?;

    Ok(())
}

/// Returns every group we're in, members included.
#[tauri::command]
fn get_groups(app: AppHandle) -> Result<Vec<Group>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let mut group_statement = conn
        .prepare("SELECT id, name FROM groups ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;

    let rows = group_statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;

    let mut groups = Vec::new();
    for row in rows {
        let (id, name) = row.map_err(|e| e.to_string())?;
        let members = get_group_members(&conn, &id).map_err(|e| e.to_string())?;

        groups.push(Group { id, name, members });
    }

    Ok(groups)
}

/// Returns the peer IDs in one group.
fn get_group_members(conn: &Connection, group_id: &str) -> SqlResult<Vec<String>> {
    let mut statement = conn.prepare("SELECT peer_id FROM group_members WHERE group_id = ?1")?;

    let rows = statement.query_map([group_id], |row| row.get::<_, String>(0))?;

    let mut members = Vec::new();
    for row in rows {
        members.push(row?);
    }

    Ok(members)
}

/// Leaves a group: forgets it, its members, and the whole conversation.
///
/// Returns how many messages were deleted. Local only — the other members keep
/// talking, they just stop hearing from us. Unsubscribing from the topic is the
/// frontend's job, since that needs the network task.
#[tauri::command]
fn delete_group(app: AppHandle, group_id: String) -> Result<usize, String> {
    let mut conn = get_db_connection(&app).map_err(|e| e.to_string())?;
    let transaction = conn.transaction().map_err(|e| e.to_string())?;

    let deleted_messages = transaction
        .execute("DELETE FROM messages WHERE group_id = ?1", [&group_id])
        .map_err(|e| e.to_string())?;

    transaction
        .execute("DELETE FROM group_members WHERE group_id = ?1", [&group_id])
        .map_err(|e| e.to_string())?;

    transaction
        .execute("DELETE FROM groups WHERE id = ?1", [&group_id])
        .map_err(|e| e.to_string())?;

    transaction.commit().map_err(|e| e.to_string())?;

    Ok(deleted_messages)
}

/// Returns how many messages are stored for one group.
#[tauri::command]
fn count_group_messages(app: AppHandle, group_id: String) -> Result<usize, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(1) FROM messages WHERE group_id = ?1",
            [&group_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    Ok(count.max(0) as usize)
}

/// Writes a group message to the database, keeping what's already there on a
/// clash — see `save_chat_message` for why that matters.
///
/// The reasoning applies with more force here: a group message passes through
/// every member, so any of them sees the ids in it.
///
/// `peer_id` is stored empty: for a group the conversation is identified by
/// `group_id`, and leaving `peer_id` blank keeps these rows out of every direct
/// conversation's history.
///
/// Returns whether the message was stored, the same as `save_chat_message`. This
/// is also what makes gossipsub's duplicate deliveries harmless.
#[tauri::command]
fn save_group_message(
    app: AppHandle,
    id: String,
    group_id: String,
    sender: String,
    text: String,
    status: String,
) -> Result<bool, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let rows_written = conn
        .execute(
            "INSERT OR IGNORE INTO messages (id, peer_id, group_id, sender, text, status)
             VALUES (?1, '', ?2, ?3, ?4, ?5)",
            (&id, &group_id, &sender, &text, &status),
        )
        .map_err(|e| e.to_string())?;

    Ok(rows_written > 0)
}

/// Returns one group conversation, oldest message first.
#[tauri::command]
fn get_group_history(app: AppHandle, group_id: String) -> Result<Vec<ChatMessage>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare(
            "SELECT id, peer_id, group_id, sender, text, status
             FROM messages
             WHERE group_id = ?1
             ORDER BY timestamp ASC",
        )
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map([&group_id], read_chat_message)
        .map_err(|e| e.to_string())?;

    collect_chat_messages(rows)
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
    let network_tx = network_sender(&state)?;

    let command = NetworkCommand::SendMessage { peer_id, message };
    network_tx.send(command).await.map_err(|e| e.to_string())?;

    Ok(())
}

/// Takes a copy of the channel into the network task.
///
/// Kept separate so the lock is released before any await: holding a
/// `std::sync::Mutex` across one would block the async runtime.
fn network_sender(state: &State<'_, NetworkState>) -> Result<mpsc::Sender<NetworkCommand>, String> {
    let guard = state.network_tx.lock().map_err(|e| e.to_string())?;

    Ok(guard.clone())
}

/// Starts receiving a group's messages.
///
/// Must be called for every group we're in, on startup and whenever one is
/// created or joined. Gossipsub delivers only to subscribers, so a group we
/// aren't subscribed to is one we simply never hear from.
#[tauri::command]
async fn subscribe_group(state: State<'_, NetworkState>, group_id: String) -> Result<(), String> {
    let network_tx = network_sender(&state)?;

    network_tx
        .send(NetworkCommand::SubscribeToGroup { group_id })
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Stops receiving a group's messages, on leaving it.
#[tauri::command]
async fn unsubscribe_group(state: State<'_, NetworkState>, group_id: String) -> Result<(), String> {
    let network_tx = network_sender(&state)?;

    network_tx
        .send(NetworkCommand::UnsubscribeFromGroup { group_id })
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Publishes a message to a group and waits to hear whether it went out.
///
/// Unlike `send_message` this reports real failures. The common one is
/// `InsufficientPeers`, which means nobody else is currently subscribed — the
/// group equivalent of shouting into an empty room, and worth telling the user
/// about rather than showing the message as sent.
#[tauri::command]
async fn send_group_message(
    state: State<'_, NetworkState>,
    group_id: String,
    message: String,
) -> Result<(), String> {
    let network_tx = network_sender(&state)?;
    let (result_tx, result_rx) = oneshot::channel();

    network_tx
        .send(NetworkCommand::PublishToGroup {
            group_id,
            message,
            result_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    result_rx
        .await
        .map_err(|_| "the network task stopped before it could answer".to_string())?
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

            let gossipsub_config = gossipsub::ConfigBuilder::default()
                .heartbeat_interval(GOSSIPSUB_HEARTBEAT)
                // Refuse messages that aren't signed, so the sender on an
                // inbound group message is the peer who actually wrote it and
                // not whoever passed it along.
                .validation_mode(gossipsub::ValidationMode::Strict)
                .check_explicit_peers_ticks(GOSSIPSUB_EXPLICIT_PEER_TICKS)
                .build()
                .expect("invalid gossipsub configuration");

            let groups = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )
            .expect("failed to start gossipsub");

            let ping = ping::Behaviour::new(
                ping::Config::new()
                    .with_interval(PING_INTERVAL)
                    .with_timeout(PING_TIMEOUT),
            );

            AppBehaviour {
                mdns,
                chat,
                groups,
                ping,
            }
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

/// Connects to peers mDNS has found.
///
/// Handing them to gossipsub does two jobs: it builds the mesh that group
/// messages travel over, and it dials any peer we aren't connected to yet, which
/// is what turns a discovery into the connection that presence is based on.
/// Gossipsub never goes looking for peers itself.
///
/// Peers are never handed back once added. Gossipsub retries them on a timer, so
/// keeping them means a node we briefly lost is picked up again without waiting
/// for mDNS to rediscover it. The cost is a failed dial every 30 seconds for a
/// node that really did leave, which on a local network is nothing.
fn connect_to_discovered_peers(swarm: &mut Swarm<AppBehaviour>, peers: Vec<PeerId>) {
    for peer_id in peers {
        swarm.behaviour_mut().groups.add_explicit_peer(&peer_id);
    }
}

/// Records that a peer is reachable and tells the frontend.
///
/// Presence is deliberately based on whether a connection exists rather than on
/// mDNS records, which cannot be trusted to expire honestly.
///
/// The reason is a quirk of how mDNS refreshes work. A node's record is only
/// renewed in other nodes' tables when it *answers* a query, and receiving any
/// answer resets a node's own timer for asking. Each node's query interval is
/// randomised once at startup and then never changes, so after the first round
/// every node's timer restarts together and whichever node drew the shortest
/// interval always asks first — forever. That node never answers anyone, so its
/// record is never renewed, and once its TTL passes it disappears from every
/// other node's list and never returns, despite being alive and connected.
///
/// A connection, by contrast, says exactly what we want to know: the peer is
/// there right now. It also detects a node going away immediately, since a
/// process that exits closes its sockets.
fn handle_peer_connected(app: &AppHandle, peer_id: PeerId) {
    let state = app.state::<NetworkState>();
    let peer_id = peer_id.to_string();

    match state.active_peers.lock() {
        Ok(mut active_peers) => {
            // Nothing new to report on a second connection to the same peer.
            if !active_peers.insert(peer_id.clone()) {
                return;
            }
        }
        Err(error) => eprintln!("could not record connected peer: {}", error),
    }

    emit_to_frontend(app, "peer-discovered", peer_id);
}

/// Records that a peer is no longer reachable and tells the frontend.
///
/// Only called once the last connection to that peer is gone.
fn handle_peer_disconnected(app: &AppHandle, peer_id: PeerId) {
    let state = app.state::<NetworkState>();
    let peer_id = peer_id.to_string();

    match state.active_peers.lock() {
        Ok(mut active_peers) => {
            active_peers.remove(&peer_id);
        }
        Err(error) => eprintln!("could not forget disconnected peer: {}", error),
    }

    emit_to_frontend(app, "peer-lost", peer_id);
}

/// Handles one inbound group message.
///
/// Held to the same rule as direct messages: if the author isn't a contact, it
/// goes no further. Gossipsub will still relay it onward to other members, which
/// is what keeps the mesh working for people who do know them.
fn handle_group_message(app: &AppHandle, message: gossipsub::Message) {
    // Strict validation guarantees a signature, so this is the peer who wrote
    // the message rather than whoever forwarded it.
    let Some(sender) = message.source else {
        return;
    };

    let sender_id = sender.to_string();
    if !is_contact(app, &sender_id) {
        return;
    }

    let Some(group_id) = group_id_from_topic(message.topic.as_str()) else {
        return;
    };

    let payload = match String::from_utf8(message.data) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("ignoring a group message that isn't text: {}", error);
            return;
        }
    };

    emit_to_frontend(
        app,
        "group-message-received",
        GroupPayload {
            group_id: group_id.to_string(),
            sender: sender_id,
            message: payload,
        },
    );
}

/// Subscribes to every group already in the database.
///
/// Runs once at startup: without it, a restart would leave us in groups we no
/// longer hear anything from.
fn subscribe_to_saved_groups(app: &AppHandle, swarm: &mut Swarm<AppBehaviour>) {
    let groups = match get_groups(app.clone()) {
        Ok(groups) => groups,
        Err(error) => {
            eprintln!("could not load groups to subscribe to: {}", error);
            return;
        }
    };

    for group in groups {
        if let Err(error) = swarm.behaviour_mut().groups.subscribe(&group_topic(&group.id)) {
            eprintln!("could not subscribe to group '{}': {}", group.name, error);
        }
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
            connect_to_discovered_peers(swarm, peers.collect());
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Mdns(mdns::Event::Expired(_))) => {
            // Ignored on purpose. An mDNS record expiring says only that we
            // haven't been told about the peer lately, which happens routinely to
            // a node that is alive and connected — see `handle_peer_connected`.
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Ping(ping::Event {
            peer,
            connection,
            result: Err(failure),
        })) => {
            // libp2p leaves the policy to us: a failed ping is reported, not
            // acted on. Closing the connection is what turns it into a presence
            // change, since that path already tells the frontend.
            //
            // Safe to be decisive about. If the peer is in fact alive, gossipsub
            // retries it within 30 seconds and it comes straight back.
            eprintln!("ping to {} failed ({}), closing the connection", peer, failure);
            swarm.close_connection(connection);
        }

        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            handle_peer_connected(app, peer_id);
        }

        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established,
            ..
        } => {
            // Peers can hold more than one connection; they're only gone when
            // the last one closes.
            if num_established == 0 {
                handle_peer_disconnected(app, peer_id);
            }
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Groups(gossipsub::Event::Message {
            message,
            ..
        })) => {
            handle_group_message(app, message);
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

        NetworkCommand::SubscribeToGroup { group_id } => {
            if let Err(error) = swarm.behaviour_mut().groups.subscribe(&group_topic(&group_id)) {
                eprintln!("could not subscribe to group '{}': {}", group_id, error);
            }
        }

        NetworkCommand::UnsubscribeFromGroup { group_id } => {
            swarm
                .behaviour_mut()
                .groups
                .unsubscribe(&group_topic(&group_id));
        }

        NetworkCommand::PublishToGroup {
            group_id,
            message,
            result_tx,
        } => {
            let result = swarm
                .behaviour_mut()
                .groups
                .publish(group_topic(&group_id), message.into_bytes())
                .map(|_message_id| ())
                .map_err(|error| error.to_string());

            // The caller is gone if the window closed mid-send; nothing to do.
            let _ = result_tx.send(result);
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

    subscribe_to_saved_groups(&app, &mut swarm);

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
            // Groups
            save_group,
            get_groups,
            delete_group,
            count_group_messages,
            save_group_message,
            get_group_history,
            // Network
            get_active_peers,
            send_message,
            subscribe_group,
            unsubscribe_group,
            send_group_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
