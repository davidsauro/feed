//! A node that carries traffic for other nodes.
//!
//! Most people run this application from behind a home router, which will not
//! accept incoming connections. Two such nodes can never reach each other
//! however hard they try. This one sits somewhere reachable and passes messages
//! between them: every node dials *out* to it, which routers do allow, and
//! gossipsub does the rest.
//!
//! It is deliberately ignorant of what it carries. Messages are sealed for their
//! recipients before they leave the sending node, so what passes through here is
//! ciphertext this program has no way to read, and no reason to want to. It does
//! not know what a contact is, what a group is, or which conversation a topic
//! belongs to. That ignorance is the point: it means running one of these for
//! your friends does not ask them to trust you with what they say.
//!
//! What it can see is who is talking to whom, and when. A conversation is named
//! by an opaque string, but two clients interested in the same string are two
//! clients with something to say to each other. That is inherent to routing
//! anything at all, and worth being honest about with anyone whose traffic you
//! carry.

mod config;
mod identity;
mod mirror;

use std::env;
use std::error::Error;
use std::path::PathBuf;

use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{allow_block_list, connection_limits, gossipsub, noise, tcp, yamux, Multiaddr, PeerId};

use crate::config::Config;
use crate::mirror::{Caps, Decision, Mirror};

/// Everything this server does on the network.
#[derive(NetworkBehaviour)]
struct ServerBehaviour {
    /// Refuses connections beyond what this server is willing to hold. Placed
    /// first so a connection is turned away before anything else spends effort
    /// on it.
    limits: connection_limits::Behaviour,

    /// The allowlist, when there is one. Off entirely for an open server, which
    /// is the default.
    ///
    /// This can only be applied once a connection has been established, because
    /// a peer's identity isn't known until the handshake proves it — there is no
    /// earlier moment at which there would be anything to check.
    allowed: Toggle<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>,

    /// The one protocol that matters here: it carries every message this server
    /// passes along, for the conversations it has been asked to carry.
    gossipsub: gossipsub::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(config::DEFAULT_PATH));

    let config = Config::load(&path)?;
    let keypair = identity::load_or_create(&config.identity_file)?;

    println!("this server is {}", keypair.public().to_peer_id());

    // Siblings are allowed automatically. Forgetting to list them is the obvious
    // way to close a server and quietly cut it off from the others at the same
    // time.
    let siblings = parse_siblings(&config.siblings)?;
    let allowed = build_allowlist(&config, &siblings)?;

    if config.is_open() {
        println!("open: any peer may connect");
    } else {
        println!(
            "allowing {} peer(s), including {} sibling server(s)",
            config.allowed_peers.len() + siblings.len(),
            siblings.len()
        );
    }

    let limits = connection_limits::ConnectionLimits::default()
        .with_max_established_per_peer(Some(config.limits.max_connections_per_peer))
        .with_max_pending_incoming(Some(config.limits.max_pending_incoming))
        .with_max_pending_outgoing(Some(config.limits.max_pending_outgoing))
        .with_max_established_incoming(Some(config.limits.max_connections_incoming))
        .with_max_established_outgoing(Some(config.limits.max_connections_outgoing))
        .with_max_established(Some(config.limits.max_connections_total));

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            // Shared with the app rather than written out again here. Two nodes
            // that disagree about this don't fail to start, they fail to talk.
            let gossipsub_config = feed_protocol::gossipsub_config()
                .expect("the shared gossipsub configuration is not valid");

            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub_config,
            )
            .expect("failed to start gossipsub");

            ServerBehaviour {
                limits: connection_limits::Behaviour::new(limits),
                allowed,
                gossipsub,
            }
        })?
        .build();

    for address in &config.listen_on {
        swarm.listen_on(address.parse()?)?;
    }

    for (peer, address) in &siblings {
        // Registered as explicit peers as well as dialled, so a sibling that is
        // down when we start is picked up when it comes back.
        swarm.behaviour_mut().gossipsub.add_explicit_peer(peer);

        if let Err(error) = swarm.dial(address.clone()) {
            eprintln!("could not reach sibling {}: {}", address, error);
        }
    }

    let mut mirror = Mirror::new(Caps {
        per_peer: config.limits.max_topics_per_peer,
        total: config.limits.max_topics_total,
    });

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                // The address a client needs includes this server's identity, so
                // print the whole thing rather than making anyone assemble it.
                println!(
                    "listening on {}/p2p/{}",
                    address,
                    swarm.local_peer_id()
                );
            }

            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("{} connected", peer_id);
            }

            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                ..
            } => {
                // Only when the last connection to them is gone: a peer that
                // still has another one open hasn't left.
                if num_established == 0 {
                    for topic in mirror.forget(&peer_id) {
                        stop_carrying(&mut swarm, &topic);
                    }

                    println!(
                        "{} disconnected, carrying {} conversation(s)",
                        peer_id,
                        mirror.carried()
                    );
                }
            }

            SwarmEvent::Behaviour(ServerBehaviourEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic },
            )) => match mirror.wanted(peer_id, topic.clone()) {
                Decision::Subscribe => {
                    // Subscribing is what makes this server a relay for the
                    // conversation. Merely knowing somebody is interested would
                    // leave it outside the mesh, receiving nothing to pass on.
                    let ident = gossipsub::IdentTopic::new(topic.to_string());

                    if let Err(error) = swarm.behaviour_mut().gossipsub.subscribe(&ident) {
                        eprintln!("could not carry {}: {}", topic, error);
                    } else {
                        println!("carrying {} ({} total)", topic, mirror.carried());
                    }
                }

                Decision::AlreadyCarried => {}

                Decision::Refused(reason) => {
                    println!("declined {} for {}: {:?}", topic, peer_id, reason);
                }
            },

            SwarmEvent::Behaviour(ServerBehaviourEvent::Gossipsub(
                gossipsub::Event::Unsubscribed { peer_id, topic },
            )) => {
                if let Some(dropped) = mirror.no_longer_wanted(&peer_id, &topic) {
                    stop_carrying(&mut swarm, &dropped);
                }
            }

            // Nothing to do. Gossipsub forwards messages on subscribed topics by
            // itself, and this server could not read one if it tried.
            SwarmEvent::Behaviour(ServerBehaviourEvent::Gossipsub(
                gossipsub::Event::Message { .. },
            )) => {}

            _ => {}
        }
    }
}

/// Stops carrying a conversation nobody here wants any more.
fn stop_carrying(swarm: &mut libp2p::Swarm<ServerBehaviour>, topic: &gossipsub::TopicHash) {
    let ident = gossipsub::IdentTopic::new(topic.to_string());
    swarm.behaviour_mut().gossipsub.unsubscribe(&ident);

    println!("stopped carrying {}", topic);
}

/// Reads the sibling addresses, each of which must name the server it points at.
///
/// An address without a peer id is rejected rather than dialled: connecting to
/// whatever answers at a hostname is exactly what including the identity is
/// meant to prevent.
fn parse_siblings(addresses: &[String]) -> Result<Vec<(PeerId, Multiaddr)>, String> {
    let mut siblings = Vec::new();

    for address in addresses {
        let parsed: Multiaddr = address
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

        siblings.push((peer, parsed));
    }

    Ok(siblings)
}

/// Builds the allowlist, or leaves it switched off for an open server.
fn build_allowlist(
    config: &Config,
    siblings: &[(PeerId, Multiaddr)],
) -> Result<Toggle<allow_block_list::Behaviour<allow_block_list::AllowedPeers>>, String> {
    if config.is_open() {
        return Ok(Toggle::from(None));
    }

    let mut list = allow_block_list::Behaviour::default();

    for peer in &config.allowed_peers {
        let parsed: PeerId = peer
            .parse()
            .map_err(|error| format!("{} is not a peer id: {}", peer, error))?;

        list.allow_peer(parsed);
    }

    for (peer, _) in siblings {
        list.allow_peer(*peer);
    }

    Ok(Toggle::from(Some(list)))
}
