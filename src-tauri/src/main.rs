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

mod file_crypto;
mod file_transfer;
mod group_crypto;

use feed_protocol::{DIRECT_TOPIC_PREFIX, GROUP_TOPIC_PREFIX};
use futures::stream::StreamExt;
use libp2p::gossipsub;
use libp2p::identity::Keypair;
use libp2p::ping;
use libp2p::relay;
use libp2p::request_response::cbor;
use libp2p::request_response::Config as RequestResponseConfig;
use libp2p::request_response::Event as RequestResponseEvent;
use libp2p::request_response::Message as RequestResponseMessage;
use libp2p::request_response::ProtocolSupport;
use libp2p::request_response::ResponseChannel;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::DialError;
use libp2p::core::transport::ListenerId;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::SwarmEvent;
use libp2p::{mdns, noise, tcp, yamux};
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, SwarmBuilder};
use rusqlite::Connection;
use rusqlite::Result as SqlResult;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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

/// Name of the protocol nodes use to tell each other what to call them.
///
/// Reads oddly because it once carried chat, which now goes over gossipsub. The
/// name on the wire is left alone: changing it would stop older nodes and newer
/// ones recognising each other's names, which is a poor trade for tidiness.
const NAME_PROTOCOL: &str = "/chat/1.0.0";

/// How long to wait for an announcement to be acknowledged before giving up on
/// it. Nothing depends on the answer; this only stops a stalled exchange from
/// being held open indefinitely.
const NAME_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);


/// Listen on every network interface, and let the OS pick the port.
const LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/tcp/0";

/// How long to wait for a connection to somebody we are about to fetch a file
/// from.
///
/// Reaching a peer through a relay is a dial to the server, a circuit request,
/// and then a handshake between the two ends, so it takes noticeably longer than
/// reaching somebody on the same network.
const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a configured server we are not connected to is dialled again.
///
/// We do this ourselves rather than leaning on gossipsub's explicit peer retry,
/// which dials by peer id alone and relies on some other behaviour knowing an
/// address for that peer. On the local network mDNS supplies those addresses. A
/// server is not on the local network and nothing else knows where it is, so the
/// address has to come from us, on every attempt.
const SERVER_REDIAL_INTERVAL: Duration = Duration::from_secs(30);

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

/// A relay server this node connects to. Rows in the `servers` table.
///
/// Servers exist so two people who are not on the same network can still reach
/// each other. A server carries traffic without being able to read it, so adding
/// one is a decision about reachability rather than about trust in what it can
/// see. What it can do is stop carrying, which is why more than one may be
/// configured.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Server {
    /// The full multiaddress, including the `/p2p/<peer id>` that names it.
    address: String,
    /// Pulled out of the address so the frontend can match connection events
    /// against it without parsing multiaddresses itself.
    peer_id: String,
}

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
/// `sent_at` is when the sender says they wrote it, in milliseconds since the
/// epoch, and is what conversations are ordered by. It travels inside the
/// sealed payload, so nothing that carries the message can alter it. Null for
/// messages stored before this existed.
#[derive(Serialize, Deserialize)]
struct ChatMessage {
    id: String,
    peer_id: String,
    group_id: Option<String>,
    sender: String,
    text: String,
    status: String,
    sent_at: i64,
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

/// Emitted to the frontend as the `peer-name` event.
#[derive(Clone, Serialize)]
struct PeerNamePayload {
    peer_id: String,
    name: String,
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
    /// Start receiving a group's messages. Gossipsub only delivers to
    /// subscribers, so this has to happen before anything arrives.
    SubscribeToGroup {
        group_id: String,
    },
    /// Stop receiving a group's messages, on leaving it.
    UnsubscribeFromGroup {
        group_id: String,
    },
    /// Tell everyone currently connected what to call this node.
    AnnounceName {
        name: String,
    },
    /// Start receiving one contact's messages.
    SubscribeToDirect {
        peer_id: String,
    },
    /// Stop receiving them, on removing the contact.
    UnsubscribeFromDirect {
        peer_id: String,
    },
    /// Send to one contact, sealed for them alone.
    ///
    /// Reports back like `PublishToGroup`, and for the same reason: publishing
    /// with nobody listening is a failure the sender needs to know about.
    PublishToDirect {
        peer_id: String,
        message: String,
        result_tx: oneshot::Sender<Result<(), String>>,
    },
    /// Start connecting to a relay server, and keep trying.
    ConnectToServer {
        peer_id: PeerId,
        address: Multiaddr,
    },
    /// Stop connecting to one, on removing it.
    DisconnectFromServer {
        peer_id: PeerId,
    },
    /// Dial every configured server that is not connected, now rather than at
    /// the next scheduled attempt. Sent when somebody presses the test button.
    TestServers,
    /// Open a connection to one peer at the addresses they gave us.
    ///
    /// Needed before a file can be pulled from somebody this node cannot reach
    /// directly. Reports back so the transfer can say why it could not start,
    /// rather than failing later with something vaguer.
    DialPeer {
        peer_id: PeerId,
        addresses: Vec<Multiaddr>,
        result_tx: oneshot::Sender<Result<(), String>>,
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

/// Lets a command open a stream to a peer without going through the network
/// task.
///
/// Cloning this is how a transfer gets its own stream. The library uses one
/// stream at a time per control to provide backpressure, so a transfer takes a
/// clone rather than sharing.
pub struct FileControl(pub libp2p_stream::Control);

/// The passphrase for the encrypted database, held only in memory.
///
/// `None` means either that encryption is off, or that it is on and the database
/// hasn't been unlocked yet; `is_encryption_enabled` tells those apart. It is
/// never written to disk — forgetting it is the whole point, and the reason the
/// UI warns that losing it loses the data.
struct DatabaseKey {
    passphrase: Mutex<Option<String>>,
}

/// Shared state that Tauri hands to commands that ask for it.
struct NetworkState {
    /// Peers we hold a connection to.
    active_peers: Mutex<HashSet<String>>,
    /// What other nodes have told us to call them, by peer id.
    ///
    /// Held in memory only. These come from nodes we haven't added as contacts,
    /// so nothing they say is worth writing to disk; a node that matters will
    /// say it again the next time we connect.
    peer_names: Mutex<HashMap<String, String>>,
    /// The sending half of the channel into the background network task.
    network_tx: Mutex<mpsc::Sender<NetworkCommand>>,
    /// The receiving half, waiting for the network task to be started.
    ///
    /// Held here rather than passed straight to the task because the network
    /// doesn't start until the app is unlocked. Taking it is what starts the
    /// task, and it can only be taken once, so there is no way to start twice.
    command_rx: Mutex<Option<mpsc::Receiver<NetworkCommand>>>,
    /// How many direct connections we hold to each peer, by peer id.
    ///
    /// Kept apart from `active_peers` because the two answer different
    /// questions. That one says whether somebody is reachable at all, which is
    /// what presence means. This says whether we can reach them without a server
    /// in the middle, which is what decides how large a file may be: bytes over
    /// a local network cost nobody anything, and bytes across a relay are
    /// somebody else's bandwidth.
    ///
    /// A count rather than a set, since a peer can hold a direct connection and
    /// a relayed one at the same time and losing the direct one has to be
    /// noticed.
    direct_connections: Mutex<HashMap<String, usize>>,
    /// Addresses this node can be reached at through a relay server.
    ///
    /// Filled in as reservations are accepted. These are what contacts need in
    /// order to open a connection for a file transfer, since nobody can dial a
    /// node behind a home router directly.
    relayed_addresses: Mutex<HashSet<String>>,
    /// What we know about each configured server, keyed by its peer id.
    ///
    /// Kept here rather than in the network task because a Tauri command has to
    /// be able to read it. The network task is the only writer.
    server_health: Mutex<HashMap<String, ServerHealth>>,
}

/// What is known about one server right now.
///
/// Every field is an observation rather than a promise. A server that was
/// reachable a moment ago may not be now, which is exactly why there is a button
/// to go and look again.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ServerHealth {
    /// Whether a connection to it exists at this moment.
    connected: bool,
    /// Last measured round trip, in milliseconds.
    ///
    /// Comes from the periodic ping rather than from a request made on demand,
    /// so it is up to one ping interval old. A new connection produces one
    /// immediately, which is what makes a freshly tested server report a time
    /// straight away.
    round_trip_ms: Option<u64>,
    /// When the current connection was established, in milliseconds since the
    /// epoch. Cleared when it closes.
    connected_at: Option<i64>,
    /// Why the last attempt to reach it failed.
    ///
    /// Kept after a later success would clear `connected`, because the reason a
    /// server is unreachable is the most useful thing on the screen when one is.
    last_error: Option<String>,
}

/// A node telling another what it would like to be called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NameRequest(String);

/// The reply, empty on purpose: arriving at all is the only information in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NameAck();

/// Everything this node does on the network.
///
/// The derive macro also generates an `AppBehaviourEvent` enum with one variant
/// per field below, which is what the swarm event loop matches on.
#[derive(NetworkBehaviour)]
struct AppBehaviour {
    /// Finds other nodes on the same local network.
    mdns: mdns::tokio::Behaviour,
    /// Carries nothing but nodes telling each other what to call them.
    ///
    /// Everything people say to each other goes over `groups` below, whether it
    /// is meant for a group or one person. This is a separate protocol because
    /// an announcement has to reach somebody who has not added us, and so cannot
    /// travel on a conversation that only contacts subscribe to.
    names: cbor::Behaviour<NameRequest, NameAck>,
    /// Carries group messages, one topic per group.
    ///
    /// Unlike `chat`, this reaches members we have no direct connection to:
    /// peers in the middle relay what they receive.
    groups: gossipsub::Behaviour,
    /// Carries files, on their own streams.
    ///
    /// Separate from everything else so that a large file cannot delay a
    /// message, and so a transfer gets flow control from the stream rather than
    /// having to invent it.
    files: libp2p_stream::Behaviour,
    /// Lets this node be reached through a relay server, and reach others that
    /// way.
    ///
    /// Only file transfers need it. Messages travel as gossipsub topics, which a
    /// server carries without either end holding a connection to the other. A
    /// transfer needs a real byte stream, and two nodes behind home routers have
    /// no way to open one, so a server proxies it.
    relay: relay::client::Behaviour,

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

/// The gossipsub topic our conversation with one peer travels on.
///
/// Direct messages go the same way as group messages rather than over a
/// connection to the peer itself. That is what lets them work between two nodes
/// that can never connect to each other — behind separate home routers, say —
/// as long as both can reach something in the middle. Nobody has to accept an
/// incoming connection.
fn direct_topic(keypair: &Keypair, peer: &PeerId) -> Result<gossipsub::IdentTopic, String> {
    let name = group_crypto::direct_topic_id(keypair, peer)?;

    Ok(gossipsub::IdentTopic::new(format!(
        "{}{}",
        DIRECT_TOPIC_PREFIX, name
    )))
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

        // Also applied on load, not just on creation, so a key written by an
        // older version of the app is tightened up rather than left as it was.
        restrict_to_owner(&key_path);

        return Ok(keypair);
    }

    let keypair = Keypair::generate_ed25519();
    let bytes = keypair.to_protobuf_encoding().map_err(|e| e.to_string())?;
    fs::write(&key_path, bytes).map_err(|e| e.to_string())?;
    restrict_to_owner(&key_path);

    Ok(keypair)
}

/// Restricts a file to the account that owns it.
///
/// Both files this is used on are worth keeping to ourselves: the identity key
/// is the whole of this node's authority on the network — anyone who copies it
/// can be this node — and the database is the whole of its history. They were
/// being written world-readable, which on a shared machine means every other
/// account could take one or read the other.
///
/// Reported rather than fatal: a file we couldn't tighten is still usable, and
/// refusing to start would be a worse outcome than a warning.
///
/// Unix only. Windows models permissions differently, and Tauri's data directory
/// there is already inside the user's own profile.
fn restrict_to_owner(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // Owner read and write, nothing for anyone else.
        if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            eprintln!(
                "could not restrict permissions on {}: {}",
                path.display(),
                error
            );
        }
    }

    #[cfg(not(unix))]
    let _ = path;
}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// Where this node's data lives on disk.
fn app_data_dir(app: &AppHandle) -> std::path::PathBuf {
    app.path()
        .app_local_data_dir()
        .expect("no local data directory for this app")
}

/// This node's database file.
fn db_path(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join(format!("contacts_{}.db", current_node_id()))
}

/// Marker file recording that the database is encrypted.
///
/// A flag on the filesystem rather than a row in the database, because we have to
/// know whether a passphrase is needed *before* we can read anything.
fn encryption_marker_path(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join(format!("encrypted_{}.flag", current_node_id()))
}

/// Whether this node's database is encrypted.
fn is_encryption_enabled(app: &AppHandle) -> bool {
    encryption_marker_path(app).exists()
}

/// Unlocks a connection with the passphrase held in memory.
///
/// SQLCipher requires the key before anything else touches the connection, so
/// this runs immediately after opening.
fn apply_key(conn: &Connection, passphrase: &str) -> SqlResult<()> {
    conn.pragma_update(None, "key", passphrase)
}

/// Checks that a connection can actually read the database.
///
/// With the wrong key SQLCipher fails on the first read rather than at the point
/// the key was set, so this is what turns a bad passphrase into an error.
fn verify_readable(conn: &Connection) -> SqlResult<()> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|_| ())
}

/// Opens this node's SQLite database, creating the file if it isn't there yet.
///
/// Connections are cheap and are not thread safe, so every caller opens its own
/// rather than sharing one.
///
/// If a passphrase is held in memory it is applied here, which is what makes
/// every command work the same whether or not encryption is on. While the
/// database is encrypted and still locked there is no passphrase to apply, so
/// reads fail — including the contact lookup that admits inbound messages, which
/// means a locked node quietly accepts nothing. That is the intended behavior:
/// nothing should be written to a database we can't read.
fn get_db_connection(app: &AppHandle) -> SqlResult<Connection> {
    let conn = Connection::open(db_path(app))?;

    if let Some(passphrase) = stored_passphrase(app) {
        apply_key(&conn, &passphrase)?;
    }

    Ok(conn)
}

/// The passphrase for this session, if the database has been unlocked.
fn stored_passphrase(app: &AppHandle) -> Option<String> {
    // `inner` ties the reference to the app rather than to the temporary handle
    // that `state` returns, which would otherwise be dropped too early.
    let state: &DatabaseKey = app.state::<DatabaseKey>().inner();
    match state.passphrase.lock() {
        Ok(passphrase) => passphrase.clone(),
        Err(error) => {
            eprintln!("could not read the database passphrase: {}", error);
            None
        }
    }
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

    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            id          TEXT PRIMARY KEY,
            peer_id     TEXT NOT NULL,
            direction   TEXT NOT NULL,
            name        TEXT NOT NULL,
            size        INTEGER NOT NULL,
            hash        TEXT NOT NULL,
            key         TEXT NOT NULL,
            path        TEXT,
            status      TEXT NOT NULL,
            transferred INTEGER NOT NULL DEFAULT 0,
            error       TEXT,
            sent_at     INTEGER NOT NULL,
            seen        INTEGER NOT NULL DEFAULT 0
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS servers (
            address  TEXT PRIMARY KEY,
            peer_id  TEXT NOT NULL,
            added_at INTEGER NOT NULL
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
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
    add_column_if_missing(conn, "messages", "group_id", "TEXT")?;
    add_column_if_missing(conn, "messages", "sent_at", "INTEGER")?;
    // A database that predates this column has files in it that were received
    // before anything tracked being looked at. Backfilling them as unseen would
    // announce every file the user has ever received as new, so they start as
    // seen and only genuinely new arrivals raise the badge.
    add_column_if_missing(conn, "files", "addresses", "TEXT")?;

    if add_column_if_missing(conn, "files", "seen", "INTEGER NOT NULL DEFAULT 0")? {
        conn.execute("UPDATE files SET seen = 1", ())?;
    }

    Ok(())
}

/// Adds one column to a table that predates it, reporting whether it did.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> SqlResult<bool> {
    let present = {
        let mut statement = conn.prepare(&format!("PRAGMA table_info({})", table))?;

        // Column 1 of table_info is the column name.
        let names = statement.query_map([], |row| row.get::<_, String>(1))?;

        let mut present = HashSet::new();
        for name in names {
            present.insert(name?);
        }
        present
    };

    if present.contains(column) {
        return Ok(false);
    }

    conn.execute(
        &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, declaration),
        (),
    )?;

    Ok(true)
}

/// Orders a conversation by when each message was written rather than when it
/// reached us.
///
/// Those were the same thing when every message crossed one hop of a local
/// network. Once messages travel by different routes they can arrive out of
/// order, and a conversation that reads in the wrong order is worse than one
/// that arrives slowly.
///
/// Rows written before `sent_at` existed fall back to their arrival time, which
/// is the best that can be said about them.
const EFFECTIVE_SENT_AT: &str =
    "COALESCE(sent_at, CAST(strftime('%s', timestamp) AS INTEGER) * 1000)";

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

/// One server and everything currently known about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServerStatus {
    address: String,
    peer_id: String,
    #[serde(flatten)]
    health: ServerHealth,
}

/// How long a manual test waits before reporting what it found.
///
/// Long enough for a dial to a server across the internet to either complete or
/// fail, and for the first ping on a new connection to come back. Short enough
/// that somebody who pressed a button is not left wondering.
const SERVER_TEST_WAIT: Duration = Duration::from_secs(6);

/// Checks every configured server and reports what came back.
///
/// Dials the ones that are not connected rather than only reporting what is
/// already known, which is the difference between a test and a status readout.
/// A server that is already connected is not disturbed: its round trip comes
/// from the ping that is running anyway.
#[tauri::command]
async fn test_servers(
    app: AppHandle,
    state: State<'_, NetworkState>,
) -> Result<Vec<ServerStatus>, String> {
    let servers = get_servers(app.clone())?;

    {
        let network_tx = network_sender(&state)?;
        network_tx
            .send(NetworkCommand::TestServers)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Waiting is the honest way to do this. A dial is not instant, and reporting
    // before the answers are in would report the state we started from.
    tokio::time::sleep(SERVER_TEST_WAIT).await;

    read_server_status(&app, servers)
}

/// The current status of every configured server, without testing anything.
#[tauri::command]
fn get_server_status(app: AppHandle) -> Result<Vec<ServerStatus>, String> {
    let servers = get_servers(app.clone())?;

    read_server_status(&app, servers)
}

/// Joins the saved servers with what the network task has observed about them.
fn read_server_status(app: &AppHandle, servers: Vec<Server>) -> Result<Vec<ServerStatus>, String> {
    let state = app.state::<NetworkState>();

    let health = state
        .server_health
        .lock()
        .map_err(|e| format!("could not read server health: {}", e))?;

    Ok(servers
        .into_iter()
        .map(|server| ServerStatus {
            health: health.get(&server.peer_id).cloned().unwrap_or_default(),
            address: server.address,
            peer_id: server.peer_id,
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// One file, in one direction, between us and one other person.
///
/// The same row serves both sides. For a file we are sending, `path` is where it
/// lives on this machine and `key` is what we sealed it with. For one we are
/// receiving, `path` is where it is being written and `key` is what came in the
/// offer. `transferred` is what makes resuming possible.
#[derive(Clone, Serialize, Deserialize)]
pub struct FileTransfer {
    pub id: String,
    pub peer_id: String,
    pub direction: String,
    pub name: String,
    pub size: u64,
    pub hash: String,
    pub key: String,
    pub path: Option<String>,
    pub status: String,
    pub transferred: u64,
    pub error: Option<String>,
    pub sent_at: i64,
    /// Where the sender said they could be reached, as a JSON array.
    ///
    /// Only meaningful on an incoming transfer, and only when the sender cannot
    /// be reached directly. A node behind a home router cannot be dialled, so it
    /// tells us the relayed addresses it holds and we go through a server. Kept
    /// with the transfer so resuming after a restart still knows where to look.
    pub addresses: Option<String>,
    /// Whether this has been looked at since it arrived.
    ///
    /// Only meaningful for files coming in. Nothing arrives with a prompt, so
    /// this is what lets the interface say that something turned up while you
    /// were elsewhere.
    pub seen: bool,
}

const FILE_COLUMNS: &str =
    "id, peer_id, direction, name, size, hash, key, path, status, transferred, error, sent_at, seen, addresses";

fn read_file_row(row: &rusqlite::Row) -> SqlResult<FileTransfer> {
    Ok(FileTransfer {
        id: row.get(0)?,
        peer_id: row.get(1)?,
        direction: row.get(2)?,
        name: row.get(3)?,
        size: row.get::<_, i64>(4)?.max(0) as u64,
        hash: row.get(5)?,
        key: row.get(6)?,
        path: row.get(7)?,
        status: row.get(8)?,
        transferred: row.get::<_, i64>(9)?.max(0) as u64,
        error: row.get(10)?,
        sent_at: row.get(11)?,
        seen: row.get::<_, i64>(12)? != 0,
        addresses: row.get(13)?,
    })
}

/// Where received files are kept, one folder per person they came from.
fn files_dir(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join("files")
}

/// Finds a transfer we offered, checking it was offered to the peer asking.
///
/// Ids are random and travel only inside sealed messages, so guessing one is not
/// realistic. Checking anyway costs a comparison and means one leaked id cannot
/// be used by anybody else.
pub fn find_outgoing_file(
    app: &AppHandle,
    id: &str,
    peer_id: &str,
) -> Result<Option<FileTransfer>, String> {
    let conn = get_db_connection(app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare(&format!(
            "SELECT {} FROM files WHERE id = ?1 AND peer_id = ?2 AND direction = 'outgoing'",
            FILE_COLUMNS
        ))
        .map_err(|e| e.to_string())?;

    let mut rows = statement
        .query_map((id, peer_id), read_file_row)
        .map_err(|e| e.to_string())?;

    rows.next().transpose().map_err(|e| e.to_string())
}

pub fn find_incoming_file(app: &AppHandle, id: &str) -> Result<Option<FileTransfer>, String> {
    let conn = get_db_connection(app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare(&format!(
            "SELECT {} FROM files WHERE id = ?1 AND direction = 'incoming'",
            FILE_COLUMNS
        ))
        .map_err(|e| e.to_string())?;

    let mut rows = statement
        .query_map([id], read_file_row)
        .map_err(|e| e.to_string())?;

    rows.next().transpose().map_err(|e| e.to_string())
}

pub fn set_file_status(
    app: &AppHandle,
    id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let conn = get_db_connection(app).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE files SET status = ?1, error = ?2 WHERE id = ?3",
        (status, error, id),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

pub fn set_file_progress(app: &AppHandle, id: &str, transferred: u64) -> Result<(), String> {
    let conn = get_db_connection(app).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE files SET transferred = ?1 WHERE id = ?2",
        (transferred as i64, id),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Servers
// ---------------------------------------------------------------------------

/// The current time in milliseconds since the epoch.
///
/// Rust's clock rather than the frontend's, unlike message timestamps: these
/// values never leave the machine, so there is nothing for a remote clock to be
/// authoritative about.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

/// The relay servers this node is configured to use, oldest first.
///
/// Whether any of them is reachable is not answered here. A server is an
/// ordinary peer once connected, so the frontend already learns that from the
/// same presence events every other peer produces.
#[tauri::command]
fn get_servers(app: AppHandle) -> Result<Vec<Server>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare("SELECT address, peer_id FROM servers ORDER BY added_at ASC")
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map([], |row| {
            Ok(Server {
                address: row.get(0)?,
                peer_id: row.get(1)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut servers = Vec::new();
    for row in rows {
        servers.push(row.map_err(|e| e.to_string())?);
    }

    Ok(servers)
}

/// Saves a server and starts connecting to it.
///
/// The address is checked before it is stored, so a typo is refused at the point
/// somebody pastes it rather than becoming a connection that quietly never
/// succeeds.
#[tauri::command]
async fn add_server(
    app: AppHandle,
    state: State<'_, NetworkState>,
    address: String,
) -> Result<Server, String> {
    let (peer_id, parsed) = feed_protocol::parse_server_address(&address)?;

    // Connecting to ourselves is not useful, and the confusion it would cause is
    // worth a couple of lines to prevent.
    if peer_id.to_string() == get_identity(app.clone())? {
        return Err("that address is this node".to_string());
    }

    let server = Server {
        address: parsed.to_string(),
        peer_id: peer_id.to_string(),
    };

    {
        let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO servers (address, peer_id, added_at) VALUES (?1, ?2, ?3)",
            (&server.address, &server.peer_id, now_millis()),
        )
        .map_err(|e| e.to_string())?;
    }

    connect_to_server(&state, peer_id, parsed).await?;

    Ok(server)
}

/// Forgets a server and drops the connection to it.
#[tauri::command]
async fn remove_server(
    app: AppHandle,
    state: State<'_, NetworkState>,
    address: String,
) -> Result<(), String> {
    let (peer_id, _) = feed_protocol::parse_server_address(&address)?;

    {
        let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM servers WHERE address = ?1", (&address,))
            .map_err(|e| e.to_string())?;
    }

    let network_tx = network_sender(&state)?;
    network_tx
        .send(NetworkCommand::DisconnectFromServer { peer_id })
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Dials the servers already saved.
///
/// Called by the frontend once the database is readable, which on an encrypted
/// node is only after the passphrase has been entered. That is why this is not
/// done when the network starts.
#[tauri::command]
async fn connect_to_saved_servers(
    app: AppHandle,
    state: State<'_, NetworkState>,
) -> Result<Vec<Server>, String> {
    let servers = get_servers(app)?;

    for server in &servers {
        match feed_protocol::parse_server_address(&server.address) {
            Ok((peer_id, parsed)) => connect_to_server(&state, peer_id, parsed).await?,
            // A stored address that no longer parses should not stop the others
            // from being dialled.
            Err(error) => eprintln!("skipping a saved server: {}", error),
        }
    }

    Ok(servers)
}

/// Updates what we know about one server.
///
/// A no-op for any peer that is not a configured server, so the callers can hand
/// it every peer event without first working out which ones matter.
fn record_server_health(
    app: &AppHandle,
    peer_id: &PeerId,
    update: impl FnOnce(&mut ServerHealth),
) {
    let state = app.state::<NetworkState>();
    let peer_id = peer_id.to_string();

    match state.server_health.lock() {
        Ok(mut health) => {
            // Only servers are tracked. An entry exists from the moment one is
            // configured, so an unknown peer here is somebody else entirely.
            if let Some(entry) = health.get_mut(&peer_id) {
                update(entry);
            }
        }
        Err(error) => eprintln!("could not record server health: {}", error),
    };
}

/// Starts tracking a server, so events about it are recorded from now on.
fn begin_tracking_server(app: &AppHandle, peer_id: &PeerId) {
    let state = app.state::<NetworkState>();

    match state.server_health.lock() {
        Ok(mut health) => {
            health.entry(peer_id.to_string()).or_default();
        }
        Err(error) => eprintln!("could not track a server: {}", error),
    };
}

/// Stops tracking one, on removing it.
fn stop_tracking_server(app: &AppHandle, peer_id: &PeerId) {
    let state = app.state::<NetworkState>();

    match state.server_health.lock() {
        Ok(mut health) => {
            health.remove(&peer_id.to_string());
        }
        Err(error) => eprintln!("could not stop tracking a server: {}", error),
    };
}

/// Asks the network task to start connecting to one server.
async fn connect_to_server(
    state: &State<'_, NetworkState>,
    peer_id: PeerId,
    address: Multiaddr,
) -> Result<(), String> {
    let network_tx = network_sender(state)?;

    network_tx
        .send(NetworkCommand::ConnectToServer { peer_id, address })
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// Everything this node has sent or received, newest first.
#[tauri::command]
fn get_files(app: AppHandle) -> Result<Vec<FileTransfer>, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let mut statement = conn
        .prepare(&format!(
            "SELECT {} FROM files ORDER BY sent_at DESC",
            FILE_COLUMNS
        ))
        .map_err(|e| e.to_string())?;

    let rows = statement
        .query_map([], read_file_row)
        .map_err(|e| e.to_string())?;

    let mut files = Vec::new();
    for row in rows {
        files.push(row.map_err(|e| e.to_string())?);
    }

    Ok(files)
}

/// Marks everything that has finished arriving as looked at.
///
/// Called when the files view is opened. A transfer still in flight is left
/// alone, so it raises the badge again once it settles rather than being
/// counted as seen before it existed.
#[tauri::command]
fn mark_files_seen(app: AppHandle) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE files SET seen = 1
         WHERE direction = 'incoming' AND status IN ('complete', 'failed')",
        (),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// One file the user has picked but not yet sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PickedFile {
    path: String,
    name: String,
    size: u64,
    /// Set when this is over the ceiling, so it can be refused before sending
    /// rather than after.
    too_large: bool,
    /// Set when the path could not be read at all.
    unreadable: bool,
}

/// Looks at files the user has picked, without sending anything.
///
/// This exists so the staging list can show real sizes and can grey out a file
/// that would be refused. Reporting that at the point of picking is far kinder
/// than accepting it and failing at send time.
#[tauri::command]
async fn inspect_files(
    app: AppHandle,
    peer_id: String,
    paths: Vec<String>,
) -> Result<Vec<PickedFile>, String> {
    // Depends on who it is going to, since a file only has a size limit when it
    // has to cross somebody else's server to get there.
    let limit = size_limit_for(&app, &peer_id);
    let mut picked = Vec::new();

    for path in paths {
        let name = Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        match tokio::fs::metadata(&path).await {
            Ok(metadata) if metadata.is_file() => picked.push(PickedFile {
                path,
                name,
                size: metadata.len(),
                too_large: limit.is_some_and(|limit| metadata.len() > limit),
                unreadable: false,
            }),
            // A directory lands here too. Neither can be sent, and from the
            // list's point of view they fail the same way.
            _ => picked.push(PickedFile {
                path,
                name,
                size: 0,
                too_large: false,
                unreadable: true,
            }),
        }
    }

    Ok(picked)
}

/// Prepares a file to be sent, and returns what the offer message needs.
///
/// Nothing is sent from here. The caller puts these details in an offer, which
/// travels as an ordinary sealed message, and the recipient then asks for the
/// bytes.
#[tauri::command]
async fn send_file(
    app: AppHandle,
    peer_id: String,
    path: String,
    sent_at: i64,
) -> Result<FileTransfer, String> {
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|e| format!("could not read {}: {}", path, e))?;

    if !metadata.is_file() {
        return Err(format!("{} is not a file", path));
    }

    let size = metadata.len();

    if let Some(limit) = size_limit_for(&app, &peer_id) {
        if size > limit {
            return Err(format!(
                "that file is {:.1}MB. Sending through a relay server is limited to {}MB, and \
                 there is no limit to somebody on this network",
                size as f64 / (1024.0 * 1024.0),
                limit / (1024 * 1024)
            ));
        }
    }

    let name = std::path::Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    let hash = file_transfer::hash_file(&path).await?;
    let key = file_crypto::FileKey::generate();

    let transfer = FileTransfer {
        id: random_id(),
        peer_id,
        direction: "outgoing".to_string(),
        name,
        size,
        hash,
        key: file_transfer::encode_key(&key),
        path: Some(path),
        status: "offered".to_string(),
        transferred: 0,
        error: None,
        sent_at,
        // Ours already, so there is nothing to notice about it.
        seen: true,
        // Outgoing. We are the one being fetched from, so it is the recipient
        // who needs an address, not us.
        addresses: None,
    };

    insert_file(&app, &transfer)?;

    Ok(transfer)
}

/// A file somebody has offered us, as it arrives from the interface.
///
/// Gathered into one type rather than passed as eight arguments, which is both
/// easier to read and harder to fill in wrongly.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingOffer {
    pub peer_id: String,
    /// What we call them, which is what their folder is named after.
    pub nickname: String,
    pub id: String,
    pub name: String,
    pub size: u64,
    pub hash: String,
    pub key: String,
    pub sent_at: i64,
    /// Where the sender says they can be reached through a relay.
    ///
    /// Carried in the offer rather than announced separately, which avoids the
    /// timing problem an announcement would have: this arrives sealed, to
    /// exactly the person who needs it, at exactly the moment they need it. The
    /// offer itself is proof the sender is reachable somehow, so nothing has to
    /// be remembered between transfers.
    #[serde(default)]
    pub addresses: Vec<String>,
}

/// Opens a connection to somebody we are about to pull a file from.
///
/// A no-op when we already have one, which is the local network case. Otherwise
/// it dials the relayed addresses they sent with their offer, since a node
/// behind a home router cannot be reached any other way.
pub async fn reach_peer(
    app: &AppHandle,
    peer_id: PeerId,
    addresses: Vec<Multiaddr>,
) -> Result<(), String> {
    let network_tx = {
        let state = app.state::<NetworkState>();
        network_sender(&state)?
    };

    let (result_tx, result_rx) = oneshot::channel();

    network_tx
        .send(NetworkCommand::DialPeer {
            peer_id,
            addresses,
            result_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    // Bounded, because a dial that is neither established nor refused would
    // otherwise leave a transfer waiting forever with nothing on screen to say
    // why. Building a connection through a relay is several round trips, so this
    // is generous rather than tight.
    let answer = tokio::time::timeout(DIAL_TIMEOUT, result_rx)
        .await
        .map_err(|_| "they did not answer in time".to_string())?;

    answer.map_err(|_| "the network stopped answering".to_string())?
}

/// Packs the sender's addresses for storage, dropping anything unusable.
///
/// Filtered rather than trusted. These are strings from another node, and one
/// that cannot be parsed would only fail later at the point of dialling, where
/// the reason would be much harder to see.
fn encode_addresses(addresses: &[String]) -> Option<String> {
    let usable: Vec<&String> = addresses
        .iter()
        .filter(|address| address.parse::<Multiaddr>().is_ok())
        .collect();

    if usable.is_empty() {
        return None;
    }

    serde_json::to_string(&usable).ok()
}

/// Reads back what `encode_addresses` stored.
pub fn decode_addresses(stored: Option<&str>) -> Vec<Multiaddr> {
    let Some(stored) = stored else {
        return Vec::new();
    };

    serde_json::from_str::<Vec<String>>(stored)
        .unwrap_or_default()
        .iter()
        .filter_map(|address| address.parse().ok())
        .collect()
}

/// Records a file we have been offered and starts fetching it.
///
/// There is no prompt. Both people have added each other, which is where that
/// decision was made.
#[tauri::command]
async fn receive_file(
    app: AppHandle,
    state: State<'_, FileControl>,
    offer: IncomingOffer,
) -> Result<FileTransfer, String> {
    let IncomingOffer {
        peer_id,
        nickname,
        id,
        name,
        size,
        hash,
        key,
        sent_at,
        addresses,
    } = offer;

    // Checked here as well as by the sender. A limit only one side enforces is a
    // limit the other side may simply not implement.
    if let Some(limit) = size_limit_for(&app, &peer_id) {
        if size > limit {
            return Err(format!(
                "{} is {:.1}MB, and this node accepts at most {}MB through a relay",
                name,
                size as f64 / (1024.0 * 1024.0),
                limit / (1024 * 1024)
            ));
        }
    }

    let peer = parse_peer(&peer_id)?;

    // The sender chose this name, so it is cleaned before it touches a path.
    let folder = files_dir(&app).join(file_transfer::folder_for(&nickname, &peer_id));
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|e| format!("could not make a folder for their files: {}", e))?;

    let destination = file_transfer::available_path(&folder, &file_transfer::safe_file_name(&name));

    let transfer = FileTransfer {
        id: id.clone(),
        peer_id,
        direction: "incoming".to_string(),
        name,
        size,
        hash,
        key,
        path: Some(destination.to_string_lossy().to_string()),
        status: "pending".to_string(),
        transferred: 0,
        error: None,
        sent_at,
        seen: false,
        addresses: encode_addresses(&addresses),
    };

    insert_file(&app, &transfer)?;

    // Its own task, so a large file does not hold up anything else.
    let control = state.0.clone();
    tauri::async_runtime::spawn(file_transfer::fetch(app.clone(), control, peer, id));

    Ok(transfer)
}

fn insert_file(app: &AppHandle, transfer: &FileTransfer) -> Result<(), String> {
    let conn = get_db_connection(app).map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT OR IGNORE INTO files (id, peer_id, direction, name, size, hash, key, path, status, transferred, error, sent_at, seen, addresses)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        (
            &transfer.id,
            &transfer.peer_id,
            &transfer.direction,
            &transfer.name,
            transfer.size as i64,
            &transfer.hash,
            &transfer.key,
            &transfer.path,
            &transfer.status,
            transfer.transferred as i64,
            &transfer.error,
            transfer.sent_at,
            i64::from(transfer.seen),
            &transfer.addresses,
        ),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// A random identifier, for naming a transfer.
fn random_id() -> String {
    use chacha20poly1305::aead::rand_core::RngCore;

    let mut bytes = [0u8; 16];
    chacha20poly1305::aead::OsRng.fill_bytes(&mut bytes);

    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

// ---------------------------------------------------------------------------
// Tauri commands: encryption at rest
// ---------------------------------------------------------------------------

/// What the frontend needs to know before it can load anything.
#[derive(Serialize)]
struct EncryptionStatus {
    /// Whether the database on disk is encrypted.
    enabled: bool,
    /// Whether it can be read right now. False means the passphrase is needed.
    unlocked: bool,
}

#[tauri::command]
fn get_encryption_status(app: AppHandle) -> EncryptionStatus {
    let enabled = is_encryption_enabled(&app);

    EncryptionStatus {
        enabled,
        unlocked: !enabled || stored_passphrase(&app).is_some(),
    }
}

/// Opens the database with a passphrase, keeping it in memory if it works.
///
/// Also creates the tables, which is what makes turning encryption on before
/// there is anything to store work the same as turning it on later.
#[tauri::command]
fn unlock_database(app: AppHandle, passphrase: String) -> Result<(), String> {
    if !is_encryption_enabled(&app) {
        return Ok(());
    }

    let conn = Connection::open(db_path(&app)).map_err(|e| e.to_string())?;
    apply_key(&conn, &passphrase).map_err(|e| e.to_string())?;

    // A wrong passphrase surfaces here, as an unreadable file, rather than at the
    // point the key was set. Deliberately not passed through to the UI verbatim:
    // "file is not a database" describes the symptom, not the cause.
    verify_readable(&conn).map_err(|_| "That passphrase didn't work.".to_string())?;

    create_tables(&conn).map_err(|e| e.to_string())?;
    restrict_to_owner(&db_path(&app));

    let state = app.state::<DatabaseKey>();
    let mut stored = state.passphrase.lock().map_err(|e| e.to_string())?;
    *stored = Some(passphrase);

    Ok(())
}

/// Encrypts the database in place and remembers the passphrase for this session.
///
/// Works by exporting every row into a new encrypted file and swapping it in,
/// which is SQLCipher's own migration path. The swap only happens once the
/// export has fully succeeded, so a failure part way through leaves the original
/// untouched.
#[tauri::command]
fn enable_encryption(app: AppHandle, passphrase: String) -> Result<(), String> {
    if passphrase.is_empty() {
        return Err("A passphrase is required.".to_string());
    }

    if is_encryption_enabled(&app) {
        return Err("This database is already encrypted.".to_string());
    }

    let source = db_path(&app);
    let target = migration_path(&app);

    export_database(&source, None, &target, Some(&passphrase))?;

    fs::rename(&target, &source).map_err(|e| e.to_string())?;

    // The swapped-in file is newly created, so it carries default permissions
    // rather than the ones the database had.
    restrict_to_owner(&source);

    fs::write(encryption_marker_path(&app), ENCRYPTION_MARKER_NOTE).map_err(|e| e.to_string())?;

    let state = app.state::<DatabaseKey>();
    let mut stored = state.passphrase.lock().map_err(|e| e.to_string())?;
    *stored = Some(passphrase);

    Ok(())
}

/// Decrypts the database in place, leaving it readable without a passphrase.
///
/// Only possible while unlocked, since the current passphrase is needed to read
/// what's being copied out.
#[tauri::command]
fn disable_encryption(app: AppHandle) -> Result<(), String> {
    if !is_encryption_enabled(&app) {
        return Err("This database is not encrypted.".to_string());
    }

    let passphrase =
        stored_passphrase(&app).ok_or_else(|| "The database is locked.".to_string())?;

    let source = db_path(&app);
    let target = migration_path(&app);

    export_database(&source, Some(&passphrase), &target, None)?;

    fs::rename(&target, &source).map_err(|e| e.to_string())?;
    restrict_to_owner(&source);

    fs::remove_file(encryption_marker_path(&app)).map_err(|e| e.to_string())?;

    let state = app.state::<DatabaseKey>();
    let mut stored = state.passphrase.lock().map_err(|e| e.to_string())?;
    *stored = None;

    Ok(())
}

/// Copies one database into a new file, changing the key on the way.
///
/// `None` for either passphrase means that side is plaintext, which is what
/// makes this serve both encrypting and decrypting.
fn export_database(
    source: &std::path::Path,
    source_passphrase: Option<&str>,
    target: &std::path::Path,
    target_passphrase: Option<&str>,
) -> Result<(), String> {
    let target_name = target
        .to_str()
        .ok_or_else(|| "the database path is not valid UTF-8".to_string())?;

    // Left over from an attempt that failed before the swap.
    if target.exists() {
        fs::remove_file(target).map_err(|e| e.to_string())?;
    }

    let conn = Connection::open(source).map_err(|e| e.to_string())?;

    if let Some(passphrase) = source_passphrase {
        apply_key(&conn, passphrase).map_err(|e| e.to_string())?;
    }

    verify_readable(&conn).map_err(|e| e.to_string())?;

    // An empty key means plaintext, which is how SQLCipher spells "no
    // encryption". Both values are bound rather than interpolated, so a
    // passphrase containing quotes is no different from any other.
    conn.execute(
        "ATTACH DATABASE ?1 AS migration KEY ?2",
        (target_name, target_passphrase.unwrap_or("")),
    )
    .map_err(|e| e.to_string())?;

    let export = conn
        .query_row("SELECT sqlcipher_export('migration')", [], |_| Ok(()))
        .map_err(|e| e.to_string());

    // Detach whether or not the export worked, so the temporary file isn't left
    // open by this connection.
    let detach = conn
        .execute("DETACH DATABASE migration", [])
        .map_err(|e| e.to_string());

    export?;
    detach?;

    Ok(())
}

/// Temporary file the re-keyed copy is written to before being swapped in.
fn migration_path(app: &AppHandle) -> std::path::PathBuf {
    app_data_dir(app).join(format!("contacts_{}.db.migrating", current_node_id()))
}

/// Written into the marker file. Nothing reads it; it is there for anyone who
/// finds the file and wonders what it is.
const ENCRYPTION_MARKER_NOTE: &str =
    "This node's database is encrypted with SQLCipher. Deleting this file will \
     not decrypt it — it only records that a passphrase is needed.\n";

/// Deletes this node's database and starts over with an empty one.
///
/// The escape hatch for a forgotten passphrase: the data is unrecoverable, so
/// the only thing left is to abandon it. The identity keypair is deliberately
/// kept, so contacts who saved this node still recognise it afterwards.
#[tauri::command]
fn reset_all_data(app: AppHandle) -> Result<(), String> {
    let database = db_path(&app);

    // SQLite may leave a journal or write-ahead log beside the database. They
    // would be meaningless next to a new file, so they go too.
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let path = std::path::PathBuf::from(format!("{}{}", database.display(), suffix));

        if path.exists() {
            fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }

    let marker = encryption_marker_path(&app);
    if marker.exists() {
        fs::remove_file(&marker).map_err(|e| e.to_string())?;
    }

    let state = app.state::<DatabaseKey>();
    {
        let mut stored = state.passphrase.lock().map_err(|e| e.to_string())?;
        *stored = None;
    }

    let conn = Connection::open(&database).map_err(|e| e.to_string())?;
    create_tables(&conn).map_err(|e| e.to_string())?;
    restrict_to_owner(&database);

    Ok(())
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
// Tauri commands: this node's own name
// ---------------------------------------------------------------------------

/// Where the node's own display name is kept in the settings table.
const DISPLAY_NAME_KEY: &str = "display_name";

/// Longest display name we will advertise or accept.
///
/// Names arrive from nodes we haven't added as contacts, so this is a limit on
/// what a stranger can put on our screen as much as it is a tidiness rule.
const MAX_DISPLAY_NAME: usize = 32;

/// Returns the name this node tells others to call it, empty if never set.
#[tauri::command]
fn get_display_name(app: AppHandle) -> Result<String, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let name = conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [DISPLAY_NAME_KEY],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default();

    Ok(name)
}

/// Sets this node's own name and tells everyone currently connected.
///
/// Only a suggestion to the people who receive it: they see it beside a peer id
/// they haven't added yet, and whatever nickname they choose when adding us as a
/// contact is theirs alone and is never overwritten by this.
#[tauri::command]
async fn set_display_name(
    app: AppHandle,
    state: State<'_, NetworkState>,
    name: String,
) -> Result<(), String> {
    let name = trim_display_name(&name);

    {
        let conn = get_db_connection(&app).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            (DISPLAY_NAME_KEY, &name),
        )
        .map_err(|e| e.to_string())?;
    }

    let network_tx = network_sender(&state)?;
    network_tx
        .send(NetworkCommand::AnnounceName { name })
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Trims a display name and cuts it to length, counting characters rather than
/// bytes so a name in any script is treated the same.
fn trim_display_name(name: &str) -> String {
    name.trim().chars().take(MAX_DISPLAY_NAME).collect()
}

/// Returns the names other nodes have told us to call them, by peer id.
///
/// The frontend also listens for `peer-name`; this exists so a window that just
/// opened knows about names announced before it started listening.
#[tauri::command]
fn get_peer_names(state: State<'_, NetworkState>) -> Result<HashMap<String, String>, String> {
    let names = state.peer_names.lock().map_err(|e| e.to_string())?;

    Ok(names.clone())
}

// ---------------------------------------------------------------------------
// Tauri commands: contacts
// ---------------------------------------------------------------------------

/// Adds a contact, or renames one we already have.
#[tauri::command]
fn save_contact(app: AppHandle, peer_id: String, nickname: String) -> Result<(), String> {
    // Checked here because a peer id can now be typed in by hand rather than
    // only clicked from a list of peers already found on the network. A wrong
    // one would otherwise be saved happily and simply never reach anybody.
    let peer_id = peer_id.trim().to_string();
    parse_peer(&peer_id)?;

    if peer_id == get_identity(app.clone())? {
        return Err("that is this node's own address".to_string());
    }

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
    sent_at: i64,
) -> Result<bool, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let rows_written = conn
        .execute(
            "INSERT OR IGNORE INTO messages (id, peer_id, sender, text, status, sent_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (&id, &peer_id, &sender, &text, &status, sent_at),
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
            &format!(
                "SELECT id, peer_id, group_id, sender, text, status, {effective} AS sent_at
                 FROM messages
                 WHERE peer_id = ?1 AND group_id IS NULL
                 ORDER BY sent_at ASC",
                effective = EFFECTIVE_SENT_AT
            ),
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
        sent_at: row.get(6)?,
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
    sent_at: i64,
) -> Result<bool, String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    let rows_written = conn
        .execute(
            "INSERT OR IGNORE INTO messages (id, peer_id, group_id, sender, text, status, sent_at)
             VALUES (?1, '', ?2, ?3, ?4, ?5, ?6)",
            (&id, &group_id, &sender, &text, &status, sent_at),
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
            &format!(
                "SELECT id, peer_id, group_id, sender, text, status, {effective} AS sent_at
                 FROM messages
                 WHERE group_id = ?1
                 ORDER BY sent_at ASC",
                effective = EFFECTIVE_SENT_AT
            ),
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

/// Starts the background network task.
///
/// Called once the app is actually usable, which for an encrypted node means
/// after the passphrase has been accepted. Until then this node does not listen,
/// does not answer mDNS, and dials nobody — so it is not merely unreachable but
/// invisible, and never appears online in anyone else's contact list.
///
/// That matters because presence is based on connections. A locked node that had
/// started its network would look perfectly online while silently dropping every
/// message sent to it, which is worse than being plainly offline.
///
/// Safe to call more than once: the receiving half of the command channel can
/// only be taken once, and later calls do nothing.
#[tauri::command]
async fn start_network(app: AppHandle, state: State<'_, NetworkState>) -> Result<(), String> {
    let command_rx = {
        let mut guard = state.command_rx.lock().map_err(|e| e.to_string())?;
        guard.take()
    };

    let Some(command_rx) = command_rx else {
        return Ok(());
    };

    let keypair = get_or_create_keypair(&app_data_dir(&app), &current_node_id())?;
    let swarm = build_swarm(keypair.clone());

    // Taken before the network task starts, so a command cannot arrive and find
    // there is no way to open a stream yet.
    app.manage(FileControl(swarm.behaviour().files.new_control()));

    // This command is async for a reason that is easy to undo by accident.
    // Building the swarm sets up mDNS, which watches network interfaces through
    // netlink and needs a Tokio reactor to do it. A synchronous command runs on
    // the main thread, where there is no reactor and the whole process aborts on
    // a panic that cannot unwind. Async commands run on the async runtime, which
    // has one.

    tauri::async_runtime::spawn(run_network(app, keypair, swarm, command_rx));

    Ok(())
}

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

/// Takes a copy of the channel into the network task.
///
/// Kept separate so the lock is released before any await: holding a
/// `std::sync::Mutex` across one would block the async runtime.
fn network_sender(state: &State<'_, NetworkState>) -> Result<mpsc::Sender<NetworkCommand>, String> {
    let guard = state.network_tx.lock().map_err(|e| e.to_string())?;

    Ok(guard.clone())
}

/// Starts receiving one contact's messages.
///
/// Called for every contact once the app is unlocked, and whenever one is
/// added. A conversation we aren't listening to is one we never hear, which is
/// exactly what should happen for people who aren't contacts.
#[tauri::command]
async fn subscribe_direct(state: State<'_, NetworkState>, peer_id: String) -> Result<(), String> {
    let network_tx = network_sender(&state)?;

    network_tx
        .send(NetworkCommand::SubscribeToDirect { peer_id })
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Stops receiving one contact's messages, on removing them.
#[tauri::command]
async fn unsubscribe_direct(state: State<'_, NetworkState>, peer_id: String) -> Result<(), String> {
    let network_tx = network_sender(&state)?;

    network_tx
        .send(NetworkCommand::UnsubscribeFromDirect { peer_id })
        .await
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Sends to one contact and waits to hear whether it went out.
///
/// Success means the message reached somewhere that carries this conversation,
/// not that the recipient has it — on a local network those are the same thing,
/// but with a server in the middle they are not. The recipient's own
/// acknowledgement is what confirms delivery.
#[tauri::command]
async fn send_direct(
    state: State<'_, NetworkState>,
    peer_id: String,
    message: String,
) -> Result<(), String> {
    let network_tx = network_sender(&state)?;
    let (result_tx, result_rx) = oneshot::channel();

    network_tx
        .send(NetworkCommand::PublishToDirect {
            peer_id,
            message,
            result_tx,
        })
        .await
        .map_err(|e| e.to_string())?;

    result_rx
        .await
        .map_err(|_| "the network task stopped before it could answer".to_string())?
}

/// Marks messages left mid-flight by a previous run as failed.
///
/// A message sits at "sending" until the recipient acknowledges it. If the app
/// closes before that happens, no acknowledgement is ever coming, and the clock
/// beside it would otherwise spin forever.
#[tauri::command]
fn fail_stale_sends(app: AppHandle) -> Result<(), String> {
    let conn = get_db_connection(&app).map_err(|e| e.to_string())?;

    conn.execute(
        "UPDATE messages SET status = 'failed' WHERE status = 'sending'",
        (),
    )
    .map_err(|e| e.to_string())?;

    Ok(())
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
    let names = cbor::Behaviour::<NameRequest, NameAck>::new(
        [(
            StreamProtocol::new(NAME_PROTOCOL),
            ProtocolSupport::Full,
        )],
        RequestResponseConfig::default().with_request_timeout(NAME_REQUEST_TIMEOUT),
    );

    SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .expect("failed to configure the TCP transport")
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .expect("failed to configure the relay transport")
        .with_behaviour(|key, relay| {
            let mdns_config = mdns::Config {
                ttl: MDNS_RECORD_TTL,
                query_interval: MDNS_QUERY_INTERVAL,
                ..mdns::Config::default()
            };

            let mdns = mdns::tokio::Behaviour::new(mdns_config, key.public().to_peer_id())
                .expect("failed to start mDNS discovery");

            // Shared with the server rather than written out twice. Two nodes
            // that disagree about this don't fail to start, they fail to talk.
            let gossipsub_config = feed_protocol::gossipsub_config()
                .expect("the shared gossipsub configuration is not valid");

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
                files: libp2p_stream::Behaviour::new(),
                names,
                groups,
                relay,
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

/// Whether an address reaches a node through a relay rather than directly.
fn is_relayed(address: &Multiaddr) -> bool {
    address.iter().any(|part| part == Protocol::P2pCircuit)
}

/// Records a direct connection opening or closing.
fn count_direct_connection(app: &AppHandle, peer_id: &PeerId, change: i32) {
    let state = app.state::<NetworkState>();
    let peer_id = peer_id.to_string();

    match state.direct_connections.lock() {
        Ok(mut counts) => {
            let held = counts.entry(peer_id.clone()).or_insert(0);

            if change > 0 {
                *held += 1;
            } else {
                *held = held.saturating_sub(1);
            }

            if *held == 0 {
                counts.remove(&peer_id);
            }
        }
        Err(error) => eprintln!("could not count a direct connection: {}", error),
    };
}

/// Whether we can reach this peer without a server in the middle.
fn is_directly_connected(app: &AppHandle, peer_id: &str) -> bool {
    let state = app.state::<NetworkState>();

    let held = match state.direct_connections.lock() {
        Ok(counts) => counts.contains_key(peer_id),
        Err(error) => {
            eprintln!("could not check for a direct connection: {}", error);
            false
        }
    };

    held
}

/// The largest file that may be sent to one peer right now.
///
/// Unlimited to somebody we can reach directly, which on a local network is
/// everybody. Those bytes never leave the network and cost nobody anything, and
/// the people involved have already added each other, so there is nothing here
/// to protect anyone from.
///
/// Capped to somebody we can only reach through a relay, because then the bytes
/// are spending an operator's bandwidth rather than ours.
fn size_limit_for(app: &AppHandle, peer_id: &str) -> Option<u64> {
    if is_directly_connected(app, peer_id) {
        None
    } else {
        Some(file_crypto::MAX_RELAYED_FILE_SIZE)
    }
}

/// Adds or removes one of our own relayed addresses.
fn remember_relayed_address(app: &AppHandle, address: &Multiaddr, present: bool) {
    let state = app.state::<NetworkState>();

    match state.relayed_addresses.lock() {
        Ok(mut addresses) => {
            if present {
                addresses.insert(address.to_string());
            } else {
                addresses.remove(&address.to_string());
            }
        }
        Err(error) => eprintln!("could not record a relayed address: {}", error),
    };

    // The frontend announces these to contacts, so it has to know they changed.
    emit_to_frontend(app, "relayed-addresses-changed", ());
}

/// The addresses this node can currently be reached at through a relay.
#[tauri::command]
fn get_relayed_addresses(state: State<'_, NetworkState>) -> Result<Vec<String>, String> {
    let addresses = state
        .relayed_addresses
        .lock()
        .map_err(|e| format!("could not read the relayed addresses: {}", e))?;

    Ok(addresses.iter().cloned().collect())
}

/// The connections the network task is looking after.
///
/// Bundled rather than passed one by one, because both handlers need all of it
/// and the argument lists were getting long enough to hide a mistake.
#[derive(Default)]
struct Links {
    servers: HashMap<PeerId, ServerLink>,
    /// Callers waiting for a connection to a peer they have asked us to dial.
    ///
    /// A dial is not a connection. `Swarm::dial` returns the moment the attempt
    /// starts, so anything that answered there would be telling the caller they
    /// can reach somebody they cannot reach yet. These wait for the connection
    /// to actually be established, or for the attempt to fail.
    ///
    /// A list per peer, since two transfers from the same person can be waiting
    /// on the same connection.
    pending_dials: HashMap<PeerId, Vec<oneshot::Sender<Result<(), String>>>>,
}

impl Links {
    /// Answers everybody waiting on a connection to one peer.
    fn resolve_dials(&mut self, peer_id: &PeerId, outcome: Result<(), String>) {
        let Some(waiting) = self.pending_dials.remove(peer_id) else {
            return;
        };

        for sender in waiting {
            let _ = sender.send(outcome.clone());
        }
    }
}

/// What the network task holds about one configured server.
struct ServerLink {
    address: Multiaddr,
    /// The listener holding our reservation, once we have one.
    ///
    /// A reservation is what makes this node reachable *through* that server, so
    /// somebody who cannot dial us directly can still open a connection for a
    /// file transfer. Kept so it can be torn down when the server is removed.
    reservation: Option<ListenerId>,
}

/// Asks a server to make this node reachable through it.
///
/// Listening on `<server>/p2p-circuit` is how a reservation is requested. The
/// swarm reuses the connection we already hold to that server, so this costs
/// nothing beyond the request itself.
fn reserve_through(swarm: &mut Swarm<AppBehaviour>, address: &Multiaddr) -> Option<ListenerId> {
    let circuit = address.clone().with(Protocol::P2pCircuit);

    match swarm.listen_on(circuit.clone()) {
        Ok(listener) => Some(listener),
        Err(error) => {
            eprintln!("could not ask {} for a reservation: {}", circuit, error);
            None
        }
    }
}

/// Dials one server at the address we hold for it.
///
/// The address is passed on every attempt rather than registered once, because
/// nothing in this swarm remembers addresses for peers that mDNS did not find.
/// Dialling by peer id alone would fail with no address to try.
fn dial_server(swarm: &mut Swarm<AppBehaviour>, peer_id: PeerId, address: &Multiaddr) {
    let options = DialOpts::peer_id(peer_id)
        .addresses(vec![address.clone()])
        .build();

    if let Err(error) = swarm.dial(options) {
        // Being already connected or already dialling both land here, and
        // neither is a problem: they are the outcome this was trying to reach.
        eprintln!("not dialling {}: {}", address, error);
    }
}

/// Dials every configured server we are not already connected to.
///
/// Runs on a timer because a server that was down when the app started, or that
/// restarts later, has to be picked up again without the user doing anything.
/// Dialling one we are already connected to is refused harmlessly, but checking
/// first keeps the log quiet.
fn redial_missing_servers(swarm: &mut Swarm<AppBehaviour>, servers: &HashMap<PeerId, ServerLink>) {
    let missing: Vec<(PeerId, Multiaddr)> = servers
        .iter()
        .filter(|(peer_id, _)| !swarm.is_connected(peer_id))
        .map(|(peer_id, link)| (*peer_id, link.address.clone()))
        .collect();

    for (peer_id, address) in missing {
        dial_server(swarm, peer_id, &address);
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
/// Routes one gossipsub message to the conversation it belongs to.
///
/// Both kinds arrive here. A group topic names itself; a direct topic can't be
/// read back, so it's recognised by deriving what this sender's conversation
/// with us would be called and seeing whether that is where the message landed.
fn handle_gossipsub_message(app: &AppHandle, keypair: &Keypair, message: gossipsub::Message) {
    // Strict validation guarantees a signature, so this is the peer who wrote
    // the message rather than whoever forwarded it.
    let Some(sender) = message.source else {
        return;
    };

    let sender_id = sender.to_string();
    if !is_contact(app, &sender_id) {
        return;
    }

    let topic = message.topic.as_str().to_string();

    if topic.starts_with(DIRECT_TOPIC_PREFIX) {
        handle_direct_message(app, keypair, sender, &topic, message.data);
    } else {
        handle_group_message(app, keypair, sender, &topic, message.data);
    }
}

/// Handles one message from a contact, sent to just us.
///
/// Emitted as `chat-received`, exactly as a message arriving over a direct
/// connection would be, so everything above this is unaware of which way it
/// came.
fn handle_direct_message(
    app: &AppHandle,
    keypair: &Keypair,
    sender: PeerId,
    topic: &str,
    data: Vec<u8>,
) {
    let name = match group_crypto::direct_topic_id(keypair, &sender) {
        Ok(name) => name,
        Err(error) => {
            eprintln!("could not work out our conversation with {}: {}", sender, error);
            return;
        }
    };

    // Anything else is a conversation between two other people that we happen to
    // be carrying, and is none of our business.
    if topic != format!("{}{}", DIRECT_TOPIC_PREFIX, name) {
        return;
    }

    let plaintext = match group_crypto::open(keypair, &name, &sender, &data) {
        Ok(plaintext) => plaintext,
        Err(error) => {
            eprintln!("ignoring a message we cannot read from {}: {}", sender, error);
            return;
        }
    };

    let payload = match String::from_utf8(plaintext) {
        Ok(payload) => payload,
        Err(error) => {
            eprintln!("ignoring a message that isn't text: {}", error);
            return;
        }
    };

    emit_to_frontend(
        app,
        "chat-received",
        ChatPayload {
            sender: sender.to_string(),
            message: payload,
        },
    );
}

/// Handles one inbound group message.
fn handle_group_message(
    app: &AppHandle,
    keypair: &Keypair,
    sender: PeerId,
    topic: &str,
    data: Vec<u8>,
) {
    let sender_id = sender.to_string();

    let Some(group_id) = group_id_from_topic(topic) else {
        return;
    };

    // Only the members a message was sealed for can read it. Everyone else on
    // the topic still relays it, and gets this far, and stops here — which is
    // ordinary rather than suspicious, so it isn't reported to the user.
    let plaintext = match group_crypto::open(keypair, group_id, &sender, &data) {
        Ok(plaintext) => plaintext,
        Err(error) => {
            eprintln!("ignoring a group message we cannot read: {}", error);
            return;
        }
    };

    let payload = match String::from_utf8(plaintext) {
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

/// Builds the payload a node sends to say what it would like to be called.
fn name_announcement(name: &str) -> String {
    // Hand-built rather than via serde: it is one field, and the payload format
    // is a string the frontend parses, not a type we share.
    format!(
        "{{\"type\":\"profile\",\"name\":{}}}",
        serde_json::Value::String(name.to_string())
    )
}

/// Reads a name out of an announcement, if that is what this payload is.
///
/// Returns None for anything else, including an announcement with an empty or
/// unusable name, which is how a bad one is ignored rather than argued with.
fn announced_name(message: &str) -> Option<String> {
    let payload: serde_json::Value = serde_json::from_str(message).ok()?;

    if payload.get("type")?.as_str()? != "profile" {
        return None;
    }

    let name = trim_display_name(payload.get("name")?.as_str()?);

    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Tells one peer what to call this node, if it has been given a name.
fn announce_name_to(app: &AppHandle, swarm: &mut Swarm<AppBehaviour>, peer: PeerId) {
    let name = match get_display_name(app.clone()) {
        Ok(name) if !name.is_empty() => name,
        Ok(_) => return,
        Err(error) => {
            // Expected while the database is locked, which is never, since the
            // network only starts once it isn't.
            eprintln!("could not read this node's name: {}", error);
            return;
        }
    };

    swarm
        .behaviour_mut()
        .names
        .send_request(&peer, NameRequest(name_announcement(&name)));
}

/// Records what a peer would like to be called, and tells the frontend.
fn record_peer_name(app: &AppHandle, peer_id: &str, name: String) {
    let state = app.state::<NetworkState>();

    match state.peer_names.lock() {
        Ok(mut names) => {
            // Nothing to report if they've said the same thing before.
            if names.get(peer_id) == Some(&name) {
                return;
            }

            names.insert(peer_id.to_string(), name.clone());
        }
        Err(error) => {
            eprintln!("could not record the name of {}: {}", peer_id, error);
            return;
        }
    }

    emit_to_frontend(
        app,
        "peer-name",
        PeerNamePayload {
            peer_id: peer_id.to_string(),
            name,
        },
    );
}

/// Handles one inbound announcement.
///
/// Accepted from anyone, contact or not, because the whole point is to be
/// recognisable to somebody who hasn't added you yet. It only ever puts a name
/// beside a peer id already on screen, is capped in length, and never becomes a
/// contact or a message on its own. Names are claims, not identities — the peer
/// id remains what actually identifies a node, and stays on screen next to it.
///
/// Anything else arriving here is ignored. Conversations moved to gossipsub, so
/// nothing this version sends takes this path.
fn handle_name_announcement(
    app: &AppHandle,
    swarm: &mut Swarm<AppBehaviour>,
    sender: PeerId,
    request: NameRequest,
    response_channel: ResponseChannel<NameAck>,
) {
    let NameRequest(message) = request;

    let Some(name) = announced_name(&message) else {
        return;
    };

    record_peer_name(app, &sender.to_string(), name);

    // The reply only confirms arrival. It fails if they disconnected while we
    // were working, which is nothing to act on.
    let _ = swarm
        .behaviour_mut()
        .names
        .send_response(response_channel, NameAck());
}

/// Routes one event out of the swarm to the code that cares about it.
///
/// Events we don't act on (listening addresses, connections opening and
/// closing, and so on) fall through the catch-all arm.
fn handle_swarm_event(
    app: &AppHandle,
    swarm: &mut Swarm<AppBehaviour>,
    keypair: &Keypair,
    links: &mut Links,
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
            result: Ok(round_trip),
            ..
        })) => {
            record_server_health(app, &peer, |health| {
                health.round_trip_ms = Some(round_trip.as_millis() as u64);
            });
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Ping(ping::Event {
            peer,
            result: Err(ping::Failure::Unsupported),
            ..
        })) => {
            record_server_health(app, &peer, |health| {
                // Connected, but there is no round trip to report and there
                // never will be from this peer. Clearing it beats leaving a
                // stale number on the screen.
                //
                // Deliberately not recorded as an error. `last_error` is shown
                // as the reason a server is unreachable, and this one is
                // reachable. Putting it there would mean that if the server
                // later went down, the screen would explain its absence with
                // something that had nothing to do with it.
                health.round_trip_ms = None;
            });

            // Says the peer does not speak the ping protocol, which is a fact
            // about what it implements rather than about whether it is alive.
            // Closing on this would be badly wrong: a relay server runs
            // gossipsub and nothing else, so every server connection would be
            // dropped seconds after it was made and redialled forever.
            eprintln!("{} does not answer pings; leaving the connection alone", peer);
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Ping(ping::Event {
            peer,
            connection,
            result: Err(failure),
        })) => {
            // A timeout or a transport error, both of which do say something
            // about liveness. libp2p leaves the policy to us: a failed ping is
            // reported, not acted on. Closing the connection is what turns it
            // into a presence change, since that path already tells the
            // frontend.
            //
            // Safe to be decisive about. If the peer is in fact alive, gossipsub
            // retries it within 30 seconds and it comes straight back.
            eprintln!("ping to {} failed ({}), closing the connection", peer, failure);
            swarm.close_connection(connection);
        }

        SwarmEvent::NewListenAddr { address, .. } => {
            // Only the relayed ones matter to anyone else. A local address is of
            // no use to somebody who cannot reach this network.
            if is_relayed(&address) {
                println!("reachable through a relay at {}", address);
                remember_relayed_address(app, &address, true);
            }
        }

        SwarmEvent::ExpiredListenAddr { address, .. } => {
            if is_relayed(&address) {
                remember_relayed_address(app, &address, false);
            }
        }

        SwarmEvent::ConnectionEstablished {
            peer_id, endpoint, ..
        } => {
            links.resolve_dials(&peer_id, Ok(()));

            if !is_relayed(endpoint.get_remote_address()) {
                count_direct_connection(app, &peer_id, 1);
            }

            handle_peer_connected(app, peer_id);

            record_server_health(app, &peer_id, |health| {
                health.connected = true;
                health.connected_at = Some(now_millis());
                health.last_error = None;
            });

            // Servers are told nothing about us beyond what they must carry.
            // A server does not speak the name protocol anyway, so announcing
            // would only produce a failed negotiation, but the reason to skip it
            // is that our name is none of a relay's business.
            if !links.servers.contains_key(&peer_id) {
                // Both sides do this as they connect, so each learns what the
                // other would like to be called without either having to ask.
                announce_name_to(app, swarm, peer_id);
            }
        }

        SwarmEvent::ConnectionClosed {
            peer_id,
            num_established,
            endpoint,
            ..
        } => {
            if !is_relayed(endpoint.get_remote_address()) {
                count_direct_connection(app, &peer_id, -1);
            }

            // Peers can hold more than one connection; they're only gone when
            // the last one closes.
            if num_established == 0 {
                handle_peer_disconnected(app, peer_id);

                record_server_health(app, &peer_id, |health| {
                    health.connected = false;
                    health.connected_at = None;
                    // The round trip is left alone on purpose. It is the last
                    // measurement taken, not a claim about right now, and
                    // throwing it away would lose the only evidence of how the
                    // server behaved while it was up.
                });
            }
        }

        SwarmEvent::OutgoingConnectionError {
            peer_id: Some(peer_id),
            error,
            ..
        } => {
            links.resolve_dials(&peer_id, Err(error.to_string()));

            // Why a server could not be reached is the most useful thing there
            // is to show when one cannot. Peers that are not servers are ignored
            // by the recorder.
            record_server_health(app, &peer_id, |health| {
                health.last_error = Some(error.to_string());
            });
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Groups(gossipsub::Event::Message {
            message,
            ..
        })) => {
            handle_gossipsub_message(app, keypair, message);
        }

        SwarmEvent::Behaviour(AppBehaviourEvent::Names(RequestResponseEvent::Message {
            peer,
            message,
            ..
        })) => match message {
            RequestResponseMessage::Request {
                request, channel, ..
            } => handle_name_announcement(app, swarm, peer, request, channel),

            // Nothing waits on these. The reply to a name announcement carries
            // no information beyond having arrived, and nobody is listening for
            // it.
            RequestResponseMessage::Response { .. } => {}
        },

        _ => {}
    }
}

/// Carries out one command the frontend queued up.
fn handle_network_command(
    app: &AppHandle,
    swarm: &mut Swarm<AppBehaviour>,
    keypair: &Keypair,
    links: &mut Links,
    command: NetworkCommand,
) {
    match command {
        NetworkCommand::TestServers => {
            redial_missing_servers(swarm, &links.servers);
        }

        NetworkCommand::DialPeer {
            peer_id,
            addresses,
            result_tx,
        } => {
            if swarm.is_connected(&peer_id) {
                let _ = result_tx.send(Ok(()));
                return;
            }

            if addresses.is_empty() {
                let _ = result_tx.send(Err(
                    "they did not say where they could be reached".to_string()
                ));
                return;
            }

            match swarm.dial(DialOpts::peer_id(peer_id).addresses(addresses).build()) {
                // Started, not finished. The answer comes when the connection is
                // established or the attempt fails.
                Ok(()) => links.pending_dials.entry(peer_id).or_default().push(result_tx),

                // Somebody else is already dialling them. Wait on that one
                // rather than reporting a failure that is really a success in
                // progress.
                Err(DialError::DialPeerConditionFalse(_)) => {
                    links.pending_dials.entry(peer_id).or_default().push(result_tx)
                }

                Err(error) => {
                    let _ = result_tx.send(Err(error.to_string()));
                }
            }
        }

        NetworkCommand::ConnectToServer { peer_id, address } => {
            begin_tracking_server(app, &peer_id);
            dial_server(swarm, peer_id, &address);

            // Asked for straight away rather than after the connection is up.
            // The swarm queues it behind the dial either way, and doing it here
            // means one place decides what happens when a server is added.
            let reservation = reserve_through(swarm, &address);

            links.servers.insert(peer_id, ServerLink { address, reservation });

            // What makes the server useful. Gossipsub sends an explicit peer the
            // full list of what we are subscribed to, and forwards our messages
            // to it directly rather than only through the mesh, which is how a
            // server comes to carry our conversations without being told about
            // them in so many words.
            swarm.behaviour_mut().groups.add_explicit_peer(&peer_id);
        }

        NetworkCommand::DisconnectFromServer { peer_id } => {
            if let Some(link) = links.servers.remove(&peer_id) {
                if let Some(reservation) = link.reservation {
                    // Gives the slot back rather than leaving the server holding
                    // one for a node that is not coming back.
                    swarm.remove_listener(reservation);
                }
            }

            stop_tracking_server(app, &peer_id);
            swarm.behaviour_mut().groups.remove_explicit_peer(&peer_id);

            // An error here means we were not connected, which is the state we
            // were after.
            let _ = swarm.disconnect_peer_id(peer_id);
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

        NetworkCommand::SubscribeToDirect { peer_id } => {
            match parse_peer(&peer_id).and_then(|peer| direct_topic(keypair, &peer)) {
                Ok(topic) => {
                    if let Err(error) = swarm.behaviour_mut().groups.subscribe(&topic) {
                        eprintln!("could not listen for messages from {}: {}", peer_id, error);
                    }
                }
                Err(error) => eprintln!("cannot listen for {}: {}", peer_id, error),
            }
        }

        NetworkCommand::UnsubscribeFromDirect { peer_id } => {
            match parse_peer(&peer_id).and_then(|peer| direct_topic(keypair, &peer)) {
                Ok(topic) => {
                    swarm.behaviour_mut().groups.unsubscribe(&topic);
                }
                Err(error) => eprintln!("cannot stop listening for {}: {}", peer_id, error),
            }
        }

        NetworkCommand::PublishToDirect {
            peer_id,
            message,
            result_tx,
        } => {
            let result = publish_direct(swarm, keypair, &peer_id, &message);
            let _ = result_tx.send(result);
        }

        NetworkCommand::AnnounceName { name } => {
            let announcement = name_announcement(&name);

            // Collected first: sending borrows the swarm, and the list of who
            // to send to comes from the swarm as well.
            let connected: Vec<PeerId> = swarm.connected_peers().copied().collect();

            for peer in connected {
                swarm
                    .behaviour_mut()
                    .names
                    .send_request(&peer, NameRequest(announcement.clone()));
            }
        }

        NetworkCommand::PublishToGroup {
            group_id,
            message,
            result_tx,
        } => {
            // Sealed before it goes anywhere, so the peers that relay it carry
            // something none of them can read.
            let sealed = match seal_for_group(app, keypair, &group_id, &message) {
                Ok(sealed) => sealed,
                Err(error) => {
                    let _ = result_tx.send(Err(error));
                    return;
                }
            };

            let result = swarm
                .behaviour_mut()
                .groups
                .publish(group_topic(&group_id), sealed)
                .map(|_message_id| ())
                .map_err(describe_publish_failure);

            // The caller is gone if the window closed mid-send; nothing to do.
            let _ = result_tx.send(result);
        }
    }
}

/// Reads a peer id, with an error worth showing rather than a parse failure.
fn parse_peer(peer_id: &str) -> Result<PeerId, String> {
    peer_id
        .parse::<PeerId>()
        .map_err(|error| format!("'{}' is not a peer id: {}", peer_id, error))
}

/// Seals a message for one contact and publishes it to the conversation they
/// share with us.
///
/// The conversation's name doubles as what the encryption is bound to, so a
/// message cannot be lifted into a different conversation any more than it can
/// be lifted into a different group.
fn publish_direct(
    swarm: &mut Swarm<AppBehaviour>,
    keypair: &Keypair,
    peer_id: &str,
    message: &str,
) -> Result<(), String> {
    let peer = parse_peer(peer_id)?;
    let name = group_crypto::direct_topic_id(keypair, &peer)?;
    let sealed = group_crypto::seal(keypair, &name, &[peer], message.as_bytes())?;

    swarm
        .behaviour_mut()
        .groups
        .publish(direct_topic(keypair, &peer)?, sealed)
        .map(|_message_id| ())
        .map_err(describe_publish_failure)
}

/// Turns a publishing failure into something worth showing a person.
///
/// The common one is that nobody is listening on the conversation, which
/// gossipsub calls `NoPeersSubscribedToTopic`. That means one of two things —
/// they're offline, or they haven't added us — and since we can't tell which,
/// the wording covers both rather than guessing.
fn describe_publish_failure(error: gossipsub::PublishError) -> String {
    match error {
        gossipsub::PublishError::NoPeersSubscribedToTopic => {
            "nobody is listening for this conversation — they may be offline, or may not have \
             added you as a contact"
                .to_string()
        }
        gossipsub::PublishError::MessageTooLarge => {
            "the message is too large to send".to_string()
        }
        gossipsub::PublishError::AllQueuesFull(_) => {
            "the connection is too busy to take this right now".to_string()
        }
        other => other.to_string(),
    }
}

/// Encrypts a message for everyone else in a group.
///
/// The member list comes from our own database rather than from the caller, so
/// there is one answer to "who is in this group" and the send path can't be
/// talked into addressing someone who isn't.
///
/// Ourselves excepted: gossipsub doesn't deliver our own messages back to us,
/// and our copy is already stored locally when we send it.
fn seal_for_group(
    app: &AppHandle,
    keypair: &Keypair,
    group_id: &str,
    message: &str,
) -> Result<Vec<u8>, String> {
    let conn = get_db_connection(app).map_err(|e| e.to_string())?;
    let members = get_group_members(&conn, group_id).map_err(|e| e.to_string())?;

    let me = PeerId::from(keypair.public());
    let mut recipients = Vec::new();

    for member in members {
        match member.parse::<PeerId>() {
            Ok(peer) if peer == me => {}
            Ok(peer) => recipients.push(peer),
            Err(error) => eprintln!("ignoring the unreadable member id '{}': {}", member, error),
        }
    }

    if recipients.is_empty() {
        return Err("this group has nobody else in it".to_string());
    }

    group_crypto::seal(keypair, group_id, &recipients, message.as_bytes())
}

/// Runs the network for as long as the app is open.
///
/// Two things can wake this loop up: the swarm has news for us, or the frontend
/// has queued a command. `tokio::select!` waits on both and handles whichever
/// arrives first.
async fn run_network(
    app: AppHandle,
    keypair: Keypair,
    mut swarm: Swarm<AppBehaviour>,
    mut command_rx: mpsc::Receiver<NetworkCommand>,
) {
    // Kept for sealing and opening group messages.
    let keypair_for_crypto = keypair;

    // Inbound file transfers, each handled on its own task so that one large
    // file neither blocks another nor holds up the network loop.
    match swarm.behaviour().files.new_control().accept(file_transfer::FILE_PROTOCOL) {
        Ok(mut incoming) => {
            let app_for_files = app.clone();

            tauri::async_runtime::spawn(async move {
                while let Some((peer, stream)) = incoming.next().await {
                    tauri::async_runtime::spawn(file_transfer::serve(
                        app_for_files.clone(),
                        peer,
                        stream,
                    ));
                }
            });
        }
        Err(error) => eprintln!("could not listen for incoming files: {}", error),
    }

    let listen_address = LISTEN_ADDRESS
        .parse()
        .expect("LISTEN_ADDRESS is not a valid multiaddr");

    swarm
        .listen_on(listen_address)
        .expect("failed to start listening for connections");

    // Subscribing to saved groups is left to the frontend, which does it after
    // loading them. It can't happen here: with encryption on, the database is
    // unreadable until the user has entered their passphrase, which is long
    // after this task starts. Commands sent before this loop begins wait in the
    // channel, so nothing is missed.

    // Everything the network task looks after. Held here rather than in Tauri
    // state because only this task touches it, and a value one task owns cannot
    // be read at the wrong moment by another.
    let mut links = Links::default();

    let mut redial = tokio::time::interval(SERVER_REDIAL_INTERVAL);

    // The first tick of a Tokio interval fires immediately. Nothing is
    // configured yet at this point, so it does nothing, which is fine.
    loop {
        tokio::select! {
            swarm_event = swarm.select_next_some() => {
                handle_swarm_event(&app, &mut swarm, &keypair_for_crypto, &mut links, swarm_event);
            }

            Some(command) = command_rx.recv() => {
                handle_network_command(
                    &app,
                    &mut swarm,
                    &keypair_for_crypto,
                    &mut links,
                    command,
                );
            }

            _ = redial.tick() => {
                redial_missing_servers(&mut swarm, &links.servers);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

fn main() {
    tauri::Builder::default()
        // The picker for choosing files to send, and opening or revealing one
        // that has arrived.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_handle = app.handle().clone();

            // The channel the frontend uses to reach the network task. Both
            // halves go into Tauri state: the sending half so commands can find
            // it, the receiving half until the network is started.
            let (network_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);

            app_handle.manage(NetworkState {
                active_peers: Mutex::new(HashSet::new()),
                peer_names: Mutex::new(HashMap::new()),
                network_tx: Mutex::new(network_tx),
                command_rx: Mutex::new(Some(command_rx)),
                direct_connections: Mutex::new(HashMap::new()),
                relayed_addresses: Mutex::new(HashSet::new()),
                server_health: Mutex::new(HashMap::new()),
            });

            // Managed before anything can reach the database, since opening one
            // reads the passphrase from here.
            app_handle.manage(DatabaseKey {
                passphrase: Mutex::new(None),
            });

            // An encrypted database can't be touched until the user has entered
            // their passphrase, so setting it up waits for `unlock_database`.
            if is_encryption_enabled(&app_handle) {
                println!("database is encrypted; waiting for a passphrase");
            } else {
                match get_db_connection(&app_handle) {
                    Ok(conn) => {
                        create_tables(&conn).expect("failed to create the database tables");
                        restrict_to_owner(&db_path(&app_handle));
                    }
                    Err(error) => eprintln!("could not open the database: {}", error),
                }
            }

            // Nothing touches the network here. The frontend starts it once the
            // app is usable, which for an encrypted node is after unlocking —
            // see `start_network`.

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Encryption at rest
            get_encryption_status,
            unlock_database,
            enable_encryption,
            disable_encryption,
            reset_all_data,
            // Identity
            get_node_id,
            get_identity,
            get_display_name,
            set_display_name,
            get_peer_names,
            // Contacts
            save_contact,
            get_contacts,
            delete_contact,
            // Chat history
            save_chat_message,
            update_message_status,
            get_chat_history,
            count_chat_messages,
            // Files
            send_file,
            receive_file,
            // Servers
            get_servers,
            add_server,
            remove_server,
            connect_to_saved_servers,
            test_servers,
            get_server_status,
            get_relayed_addresses,
            // Files
            get_files,
            mark_files_seen,
            inspect_files,
            // Groups
            save_group,
            get_groups,
            delete_group,
            count_group_messages,
            save_group_message,
            get_group_history,
            // Network
            start_network,
            subscribe_direct,
            unsubscribe_direct,
            send_direct,
            fail_stale_sends,
            get_active_peers,
            subscribe_group,
            unsubscribe_group,
            send_group_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Covers the encryption migration, which is the one place in the app where a
/// mistake destroys data rather than just misbehaving.
#[cfg(test)]
mod tests {
    use super::*;

    /// A private directory for one test to work in.
    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("feed-test-{}-{}", std::process::id(), name));

        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("could not create the scratch directory");

        dir
    }

    /// Addresses from another node are strings, and one that does not parse
    /// would otherwise fail much later at the point of dialling.
    #[test]
    fn keeps_only_addresses_that_can_be_dialled() {
        let stored = encode_addresses(&[
            "/ip4/203.0.113.7/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN"
                .to_string(),
            "not an address".to_string(),
        ])
        .expect("one address was usable");

        let read_back = decode_addresses(Some(&stored));

        assert_eq!(read_back.len(), 1, "the unusable one should be dropped");
        assert!(read_back[0].to_string().contains("203.0.113.7"));
    }

    /// Nothing usable should be stored as nothing, not as an empty list, so a
    /// transfer with no addresses is plainly a transfer with no addresses.
    #[test]
    fn stores_nothing_when_no_address_is_usable() {
        assert!(encode_addresses(&[]).is_none());
        assert!(encode_addresses(&["rubbish".to_string()]).is_none());
    }

    /// An older node sends an offer with no addresses at all.
    #[test]
    fn reads_a_transfer_that_carries_no_addresses() {
        assert!(decode_addresses(None).is_empty());
        assert!(decode_addresses(Some("")).is_empty());
        assert!(decode_addresses(Some("[]")).is_empty());
    }

    /// A relayed address is what tells the app a peer is not reachable directly,
    /// which is what decides whether a size limit applies.
    #[test]
    fn recognises_an_address_that_goes_through_a_relay() {
        let direct: Multiaddr = "/ip4/192.168.1.5/tcp/4001".parse().expect("valid");
        let relayed: Multiaddr =
            "/ip4/203.0.113.7/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN/p2p-circuit"
                .parse()
                .expect("valid");

        assert!(!is_relayed(&direct));
        assert!(is_relayed(&relayed));
    }

    /// A files table as it was before arrivals were tracked.
    fn seed_files_without_seen(conn: &Connection) {
        conn.execute(
            "CREATE TABLE files (
                id          TEXT PRIMARY KEY,
                peer_id     TEXT NOT NULL,
                direction   TEXT NOT NULL,
                name        TEXT NOT NULL,
                size        INTEGER NOT NULL,
                hash        TEXT NOT NULL,
                key         TEXT NOT NULL,
                path        TEXT,
                status      TEXT NOT NULL,
                transferred INTEGER NOT NULL DEFAULT 0,
                error       TEXT,
                sent_at     INTEGER NOT NULL
            )",
            (),
        )
        .expect("could not create the old files table");

        conn.execute(
            "INSERT INTO files (id, peer_id, direction, name, size, hash, key, path, status, transferred, error, sent_at)
             VALUES ('a', 'peer', 'incoming', 'old.txt', 10, 'h', 'k', '/tmp/old.txt', 'complete', 10, NULL, 1)",
            (),
        )
        .expect("could not insert the old file");
    }

    /// Files received before this feature existed are not new arrivals.
    ///
    /// The column defaults to zero, so without the backfill an upgrade would
    /// announce everything the user has ever received as having just turned up.
    #[test]
    fn migration_treats_existing_files_as_already_seen() {
        let conn = Connection::open_in_memory().expect("could not open a database");
        seed_files_without_seen(&conn);
        conn.execute("CREATE TABLE messages (id TEXT PRIMARY KEY)", ())
            .expect("could not create the messages table");

        add_missing_columns(&conn).expect("could not migrate");

        let seen: i64 = conn
            .query_row("SELECT seen FROM files WHERE id = 'a'", [], |row| row.get(0))
            .expect("could not read the migrated row");

        assert_eq!(seen, 1);
    }

    /// Migrating twice must not undo what the second run has since recorded.
    #[test]
    fn migration_leaves_an_up_to_date_database_alone() {
        let conn = Connection::open_in_memory().expect("could not open a database");
        seed_files_without_seen(&conn);
        conn.execute("CREATE TABLE messages (id TEXT PRIMARY KEY)", ())
            .expect("could not create the messages table");

        add_missing_columns(&conn).expect("could not migrate");

        // Something arrives after the upgrade and has not been looked at.
        conn.execute("UPDATE files SET seen = 0 WHERE id = 'a'", ())
            .expect("could not mark the file unseen");

        add_missing_columns(&conn).expect("could not migrate a second time");

        let seen: i64 = conn
            .query_row("SELECT seen FROM files WHERE id = 'a'", [], |row| row.get(0))
            .expect("could not read the row");

        assert_eq!(seen, 0, "a second migration should not clear what is unseen");
    }

    /// An ordinary unencrypted database with one contact in it.
    fn seed_plaintext(path: &std::path::Path) {
        let conn = Connection::open(path).expect("could not create the database");
        create_tables(&conn).expect("could not create the tables");

        conn.execute(
            "INSERT INTO contacts (peer_id, nickname) VALUES ('peer-1', 'Ada')",
            (),
        )
        .expect("could not insert the contact");
    }

    fn stored_nickname(conn: &Connection) -> String {
        conn.query_row(
            "SELECT nickname FROM contacts WHERE peer_id = 'peer-1'",
            [],
            |row| row.get(0),
        )
        .expect("could not read the contact back")
    }

    /// Encrypting keeps the data, and only the right passphrase gets it back.
    #[test]
    fn round_trips_through_encryption() {
        let dir = scratch_dir("round-trip");
        let database = dir.join("contacts.db");
        let migration = dir.join("contacts.db.migrating");
        let passphrase = "correct horse battery staple";

        seed_plaintext(&database);

        export_database(&database, None, &migration, Some(passphrase)).expect("encrypting failed");
        fs::rename(&migration, &database).expect("could not swap in the encrypted file");

        // The right passphrase reads what was there before.
        let conn = Connection::open(&database).unwrap();
        apply_key(&conn, passphrase).unwrap();
        verify_readable(&conn).expect("the correct passphrase should open the database");
        assert_eq!(stored_nickname(&conn), "Ada");
        drop(conn);

        // No passphrase does not.
        let conn = Connection::open(&database).unwrap();
        assert!(
            verify_readable(&conn).is_err(),
            "an encrypted database must not be readable without a passphrase"
        );
        drop(conn);

        // Nor does the wrong one.
        let conn = Connection::open(&database).unwrap();
        apply_key(&conn, "not the passphrase").unwrap();
        assert!(
            verify_readable(&conn).is_err(),
            "the wrong passphrase must not open the database"
        );
        drop(conn);

        // And decrypting gives back an ordinary database.
        export_database(&database, Some(passphrase), &migration, None).expect("decrypting failed");
        fs::rename(&migration, &database).expect("could not swap in the decrypted file");

        let conn = Connection::open(&database).unwrap();
        verify_readable(&conn).expect("a decrypted database should need no passphrase");
        assert_eq!(stored_nickname(&conn), "Ada");
        drop(conn);

        let _ = fs::remove_dir_all(&dir);
    }

    /// The passphrase is bound as a parameter rather than pasted into SQL, so
    /// quotes and semicolons in it are just characters.
    #[test]
    fn accepts_an_awkward_passphrase() {
        let dir = scratch_dir("awkward");
        let database = dir.join("contacts.db");
        let migration = dir.join("contacts.db.migrating");
        let passphrase = "it's \"quoted\"; DROP TABLE contacts; --";

        seed_plaintext(&database);

        export_database(&database, None, &migration, Some(passphrase)).expect("encrypting failed");
        fs::rename(&migration, &database).expect("could not swap in the encrypted file");

        let conn = Connection::open(&database).unwrap();
        apply_key(&conn, passphrase).unwrap();
        verify_readable(&conn).expect("an awkward passphrase should still work");
        assert_eq!(stored_nickname(&conn), "Ada");
        drop(conn);

        let _ = fs::remove_dir_all(&dir);
    }

    /// A name survives the trip to another node and back.
    #[test]
    fn a_name_announcement_round_trips() {
        assert_eq!(announced_name(&name_announcement("Ada")).as_deref(), Some("Ada"));
    }

    /// A name is data, not markup: quotes and braces in one must not be able to
    /// change the shape of the payload carrying it.
    #[test]
    fn an_awkward_name_cannot_break_out_of_the_payload() {
        let awkward = r#"a","type":"chat","text":"gotcha"#;

        assert_eq!(
            announced_name(&name_announcement(awkward)).as_deref(),
            Some(awkward),
            "the name should arrive exactly as it was sent"
        );
    }

    /// Ordinary messages must not be mistaken for announcements, or they would
    /// bypass the contact check.
    #[test]
    fn only_announcements_are_read_as_names() {
        assert_eq!(announced_name(r#"{"type":"chat","id":"1","text":"hello"}"#), None);
        assert_eq!(announced_name(r#"{"type":"read","messageIds":["1"]}"#), None);
        assert_eq!(announced_name("not json at all"), None);
    }

    /// A stranger can put a name on our screen, so it has to be a short one.
    #[test]
    fn a_long_name_is_cut_down() {
        let name = announced_name(&name_announcement(&"a".repeat(500)))
            .expect("a long name should be accepted, just shortened");

        assert_eq!(name.chars().count(), MAX_DISPLAY_NAME);
    }

    /// An empty name is not a name.
    #[test]
    fn a_blank_name_is_ignored() {
        assert_eq!(announced_name(&name_announcement("   ")), None);
    }

    /// Building the swarm sets up mDNS, which watches network interfaces through
    /// netlink and needs a Tokio reactor. Doing it anywhere without one aborts
    /// the process rather than returning an error, so this at least pins that it
    /// works where it is supposed to.
    ///
    /// It does not catch the mistake that caused that abort, which was calling it
    /// from a synchronous Tauri command. Nothing here can see how a command is
    /// declared.
    #[tokio::test]
    async fn the_swarm_builds_inside_the_runtime() {
        let swarm = build_swarm(Keypair::generate_ed25519());

        assert!(swarm.behaviour().files.new_control().accept(
            file_transfer::FILE_PROTOCOL
        ).is_ok());
    }

    /// A file written by an older version is world-readable; loading it should
    /// tighten it rather than leave it that way.
    #[cfg(unix)]
    #[test]
    fn restricts_a_world_readable_file_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch_dir("permissions");
        let key = dir.join("identity.bin");

        fs::write(&key, b"not really a key").expect("could not write the file");
        fs::set_permissions(&key, fs::Permissions::from_mode(0o644))
            .expect("could not loosen the permissions");

        restrict_to_owner(&key);

        let mode = fs::metadata(&key)
            .expect("could not read the file back")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o600, "the file should be readable only by its owner");

        let _ = fs::remove_dir_all(&dir);
    }

    /// A half-finished attempt leaves a temporary file behind; the next one must
    /// not trip over it.
    #[test]
    fn overwrites_a_leftover_migration_file() {
        let dir = scratch_dir("leftover");
        let database = dir.join("contacts.db");
        let migration = dir.join("contacts.db.migrating");

        seed_plaintext(&database);
        fs::write(&migration, b"not a database").expect("could not write the leftover file");

        export_database(&database, None, &migration, Some("passphrase"))
            .expect("a leftover file should be replaced, not fatal");

        let _ = fs::remove_dir_all(&dir);
    }
}
