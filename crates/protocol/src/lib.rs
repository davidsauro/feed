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

use libp2p::multiaddr::Protocol;
use libp2p::{gossipsub, Multiaddr, PeerId};

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

/// How often gossipsub maintains its mesh and sends gossip.
///
/// The libp2p default, and it stays there. This was ten seconds for a while, on
/// the reasoning that a network this small needs less upkeep. That saved
/// nothing worth having and cost a great deal: the heartbeat also paces gossip,
/// which is how a message reaches anybody it could not be forwarded to
/// directly. Every such message waited for the next beat, so a chat message
/// took seconds to arrive on a link with a thirty millisecond round trip.
///
/// The upkeep it saved is a few comparisons over a handful of topics. Latency
/// people can feel is not worth trading for that.
pub const GOSSIPSUB_HEARTBEAT: Duration = Duration::from_secs(1);

/// How often gossipsub retries peers it has been told to stay connected to,
/// counted in heartbeats.
///
/// Thirty of them at a one second heartbeat is thirty seconds, so a peer that
/// was briefly unreachable is picked up again promptly rather than in the
/// fifty minutes the default would give.
pub const GOSSIPSUB_EXPLICIT_PEER_TICKS: u64 = 30;

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

/// Reads an address that names a server, checking that it says which one.
///
/// Used by the app for the servers a person configures, and by the server for
/// its siblings, so that both ends hold the same idea of what a server address
/// is and neither can drift into accepting something the other rejects.
///
/// The `/p2p/<peer id>` part is required rather than optional. Without it, we
/// would be connecting to whatever happens to answer at a hostname, which is the
/// whole thing including the identity is meant to prevent. With it, the
/// handshake fails unless the far end holds the matching private key, so a
/// hijacked DNS record or a machine at a recycled IP address cannot pass itself
/// off as the server people meant.
pub fn parse_server_address(address: &str) -> Result<(PeerId, Multiaddr), String> {
    let parsed: Multiaddr = address
        .trim()
        .parse()
        .map_err(|error| format!("{} is not an address: {}", address, error))?;

    let peer = parsed
        .iter()
        .find_map(|part| match part {
            Protocol::P2p(peer) => Some(peer),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "{} does not say which server it is; it needs a /p2p/<peer id> on the end",
                address
            )
        })?;

    Ok((peer, parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole address, of the shape a server prints when it starts.
    const SERVER: &str =
        "/ip4/203.0.113.7/tcp/4001/p2p/12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN";

    #[test]
    fn reads_an_address_that_names_its_server() {
        let (peer, address) = parse_server_address(SERVER).expect("should be a valid address");

        assert_eq!(
            peer.to_string(),
            "12D3KooWDpJ7As7BWAwRMfu1VU2WCqNjvq387JEYKDBj4kx6nXTN"
        );
        assert_eq!(address.to_string(), SERVER);
    }

    /// Surrounding space is what happens when somebody pastes an address.
    #[test]
    fn ignores_space_around_a_pasted_address() {
        assert!(parse_server_address(&format!("  {}\n", SERVER)).is_ok());
    }

    /// Dialling this would mean trusting whatever answers at that address.
    #[test]
    fn refuses_an_address_that_does_not_say_who_it_reaches() {
        let error = parse_server_address("/ip4/203.0.113.7/tcp/4001")
            .expect_err("an address without a peer id should be refused");

        assert!(error.contains("/p2p/"), "the error should say what is missing");
    }

    #[test]
    fn refuses_something_that_is_not_an_address() {
        assert!(parse_server_address("example.com:4001").is_err());
        assert!(parse_server_address("").is_err());
    }

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
