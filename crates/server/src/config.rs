//! What an operator decides.
//!
//! Everything has a default that works, so a server with no configuration at all
//! starts up and carries traffic for anyone. That is a deliberate choice: the
//! easiest thing to run should be a useful thing to run. An operator who wants
//! it closed says so.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Where the configuration is read from when nothing else is specified.
pub const DEFAULT_PATH: &str = "feed-server.toml";

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Addresses to listen on.
    ///
    /// The port matters: it is part of the address every client is configured
    /// with, so changing it means every client has to be updated.
    pub listen_on: Vec<String>,

    /// How this server is reached from outside, as multiaddresses without the
    /// `/p2p/` part.
    ///
    /// Needed only for relaying. When a client reserves a slot here, the reply
    /// has to contain the address other people should use to reach that client
    /// through this server. A listen address of `0.0.0.0` cannot answer that, so
    /// a server behind NAT, a container, or a DNS name has to be told.
    ///
    /// Left empty, any listen address that names a real interface is used, which
    /// covers a server bound directly to its public address. A container
    /// publishing a port does not qualify, and reservations there fail with
    /// `NoAddressesInReservation` until this is set.
    ///
    /// ```toml
    /// external_addresses = ["/dns4/relay.example.com/tcp/4001"]
    /// ```
    pub external_addresses: Vec<String>,

    /// Where this server's identity is kept.
    ///
    /// Clients address a server by its public key as well as its hostname, so
    /// losing this file makes every existing client configuration wrong. In a
    /// container it belongs on a mounted volume.
    pub identity_file: PathBuf,

    /// Peers allowed to connect, as peer ids.
    ///
    /// Empty means anyone may connect, which is the default. An operator who
    /// lists anybody is choosing to serve only those people — sibling servers
    /// included, though those are added automatically.
    pub allowed_peers: Vec<String>,

    /// Other servers to connect to, as full multiaddresses including their peer
    /// id. Traffic reaches people connected to those servers through these
    /// links.
    pub siblings: Vec<String>,

    pub limits: Limits,

    pub relay: Relay,
}

/// Whether this server will carry file transfers, and what they may cost it.
///
/// Separate from `limits` because this is a different kind of traffic. Carrying
/// conversations means forwarding small sealed messages for topics clients
/// asked for. Carrying a transfer means proxying a byte stream between two
/// people, which is bandwidth an operator spends on their behalf. Somebody
/// running an open relay should be able to say no to the second without closing
/// the first.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Relay {
    /// Whether to relay connections at all.
    ///
    /// On by default. A relay that cannot pass files is half a relay, and the
    /// caps below are the answer to abuse rather than switching it off.
    pub enabled: bool,

    /// Total bytes one relayed connection may carry before it is cut.
    ///
    /// Not a file size limit, which is the mistake this comment exists to
    /// prevent. A relayed connection carries everything between two people, in
    /// both directions, for as long as it lives: several transfers, and the
    /// conversation traffic alongside them. The count never resets. So this
    /// wants to be well clear of ordinary use, and the app's own per-file limit
    /// is what tells somebody a particular file is too big.
    ///
    /// Zero means no limit.
    pub max_circuit_bytes: u64,

    /// How long one relayed connection may live, in seconds.
    ///
    /// A hard timer from the moment it is established, not an idle timeout. The
    /// libp2p default of two minutes is far too short here: a relayed
    /// connection carries the conversation as well as any transfers, so it is
    /// meant to last as long as the two people are talking.
    pub max_circuit_duration_secs: u64,

    /// How many clients may hold a reservation, which is what makes each of them
    /// reachable through this server.
    ///
    /// Wants to be at least as large as the number of clients served, since
    /// every one of them needs one. The libp2p default of 128 is far below the
    /// connection limits above.
    pub max_reservations: usize,
}

/// What one client is allowed to cost this server.
#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Conversations one client may ask this server to carry.
    ///
    /// Not something libp2p can do for us: connection limits count connections,
    /// and a single connection can ask for any number of conversations, each of
    /// which costs memory here.
    pub max_topics_per_peer: usize,

    /// Conversations this server will carry in total.
    pub max_topics_total: usize,

    /// Connections from any one peer. Two allows a new one to be established
    /// before an old one has finished closing.
    pub max_connections_per_peer: u32,

    /// Handshakes in progress. The cheapest thing to flood a server with is
    /// connections that never finish opening.
    pub max_pending_incoming: u32,
    pub max_pending_outgoing: u32,

    /// Established connections, roughly the number of clients served.
    pub max_connections_incoming: u32,
    pub max_connections_outgoing: u32,
    pub max_connections_total: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen_on: vec!["/ip4/0.0.0.0/tcp/4001".to_string()],
            external_addresses: Vec::new(),
            identity_file: PathBuf::from("identity.bin"),
            allowed_peers: Vec::new(),
            siblings: Vec::new(),
            limits: Limits::default(),
            relay: Relay::default(),
        }
    }
}

impl Default for Relay {
    fn default() -> Self {
        Self {
            enabled: true,
            // Ten times the app's per-file limit, so somebody sending a handful
            // of files in a sitting never trips it and somebody streaming
            // without end still does.
            max_circuit_bytes: 256 * 1024 * 1024,
            max_circuit_duration_secs: 60 * 60,
            // In step with max_connections_incoming, since a client that can
            // connect but cannot reserve is reachable by nobody.
            max_reservations: 512,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_topics_per_peer: 64,
            max_topics_total: 4096,
            max_connections_per_peer: 2,
            max_pending_incoming: 64,
            max_pending_outgoing: 16,
            max_connections_incoming: 512,
            max_connections_outgoing: 32,
            max_connections_total: 550,
        }
    }
}

impl Config {
    /// Reads a configuration file, or falls back to the defaults if there isn't
    /// one.
    ///
    /// A missing file is not an error — it means "run with the defaults". A file
    /// that exists but can't be read is, because the operator meant something by
    /// it and guessing would be worse than stopping.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            println!(
                "no configuration at {}, using defaults (open to anyone)",
                path.display()
            );
            return Ok(Self::default());
        }

        let text = fs::read_to_string(path)
            .map_err(|error| format!("could not read {}: {}", path.display(), error))?;

        toml::from_str(&text)
            .map_err(|error| format!("{} is not valid configuration: {}", path.display(), error))
    }

    /// Whether an allowlist is in force.
    pub fn is_open(&self) -> bool {
        self.allowed_peers.is_empty()
    }
}
