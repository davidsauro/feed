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
            identity_file: PathBuf::from("identity.bin"),
            allowed_peers: Vec::new(),
            siblings: Vec::new(),
            limits: Limits::default(),
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
