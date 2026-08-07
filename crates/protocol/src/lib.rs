//! What the app and the server have to agree on.
//!
//! Deliberately small. The server carries traffic without understanding it: it
//! never opens a message, never learns what a topic is for, and holds no idea of
//! contacts, groups, or chat. Everything about *content* — how a message is
//! sealed, how a conversation is named, what the payloads mean — belongs to the
//! app alone and is not here.
//!
//! What is left is the shape of the network itself. Both ends run gossipsub, and
//! gossipsub only works between nodes configured compatibly; two nodes that
//! disagree about signing or message size don't fail loudly, they just quietly
//! fail to exchange anything. That is exactly the kind of drift a shared crate
//! prevents.

use std::time::Duration;

use libp2p::gossipsub;

/// Prefix for the topic a group's messages travel on.
///
/// The server can't tell one group from another, and doesn't try. It knows these
/// prefixes only so it can decline to carry topics that have nothing to do with
/// this application — otherwise anyone who found the server could use it to
/// relay whatever they liked.
pub const GROUP_TOPIC_PREFIX: &str = "/group/1.0.0/";

/// Prefix for the topic a one-to-one conversation travels on. What follows it is
/// derived from a secret only the two people involved can compute, so it means
/// nothing to anybody else — including the server.
pub const DIRECT_TOPIC_PREFIX: &str = "/direct/1.0.0/";

/// Whether a topic is one this application uses.
///
/// The server's test for whether it is willing to carry something.
pub fn is_app_topic(topic: &str) -> bool {
    topic.starts_with(GROUP_TOPIC_PREFIX) || topic.starts_with(DIRECT_TOPIC_PREFIX)
}

/// How often gossipsub maintains its mesh.
///
/// The default is every second, which is more upkeep than a network of this size
/// needs. Both ends use the same value so their meshes settle at the same pace.
pub const GOSSIPSUB_HEARTBEAT: Duration = Duration::from_secs(10);

/// How often gossipsub retries peers it has been told to stay connected to,
/// counted in heartbeats. Three of them is thirty seconds, so a peer that was
/// briefly unreachable is picked up again promptly rather than in the fifty
/// minutes the default would give at our heartbeat.
pub const GOSSIPSUB_EXPLICIT_PEER_TICKS: u64 = 3;

/// Builds the gossipsub configuration both ends use.
///
/// The setting that matters most here is strict validation. It requires every
/// message to be signed, which is what lets a receiver trust that the sender of
/// a message is who the message says it is, rather than whoever passed it along.
/// A server that relaxed this would let a relayed message be attributed to the
/// wrong person.
pub fn gossipsub_config() -> Result<gossipsub::Config, &'static str> {
    gossipsub::ConfigBuilder::default()
        .heartbeat_interval(GOSSIPSUB_HEARTBEAT)
        .validation_mode(gossipsub::ValidationMode::Strict)
        .check_explicit_peers_ticks(GOSSIPSUB_EXPLICIT_PEER_TICKS)
        .build()
        .map_err(|_| "the shared gossipsub configuration is not valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_topics_this_application_uses() {
        assert!(is_app_topic("/group/1.0.0/abc"));
        assert!(is_app_topic("/direct/1.0.0/abc"));
    }

    /// The server carries our traffic, not anybody's traffic.
    #[test]
    fn declines_anything_else() {
        assert!(!is_app_topic("/someone-elses-app/1.0.0/abc"));
        assert!(!is_app_topic("ipfs-pubsub"));
        assert!(!is_app_topic(""));
    }

    #[test]
    fn the_shared_configuration_is_usable() {
        assert!(gossipsub_config().is_ok());
    }
}
