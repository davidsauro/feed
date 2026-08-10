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
use std::time::Duration;

use futures::stream::StreamExt;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{
    allow_block_list, connection_limits, gossipsub, noise, ping, relay, tcp, yamux, Multiaddr,
    PeerId,
};

use crate::config::Config;
use crate::mirror::{Caps, Decision, Mirror};

const USAGE: &str = "\
feed-server — carries traffic between nodes that cannot reach each other

    feed-server [config file]

With no argument it looks for feed-server.toml in the working directory, and
runs with defaults if there isn't one: listening on port 4001 and carrying
traffic for anyone.

Everything it keeps — its identity, and the configuration if you supply one —
lives in the working directory, which is /data in the container image.

Full documentation: https://github.com/davidsauro/feed";

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

    /// Carries connections between two people who cannot reach each other
    /// directly, which is what makes file transfers work off the local network.
    ///
    /// Separate from gossipsub and doing a different job. Gossipsub forwards
    /// small sealed messages for conversations clients asked for. This proxies a
    /// byte stream between two peers, so that a transfer, which needs a real
    /// connection rather than a topic, has one. This server can no more read
    /// what crosses a circuit than it can read a message.
    ///
    /// Off entirely when an operator says so, in which case people on this
    /// server can still talk and cannot send each other files.
    relay: Toggle<relay::Behaviour>,

    /// Answers pings.
    ///
    /// Nothing here needs it. It is for the other end: a client measures the
    /// round trip to its servers this way, and without it a client has no way to
    /// tell a working server from one that is merely still accepting a socket.
    ///
    /// It also keeps this server from looking dead. A node that does not speak
    /// ping at all reports back as unsupported, which is easy for a client to
    /// mistake for a peer that has stopped answering.
    ping: ping::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let argument = env::args().nth(1);

    if matches!(argument.as_deref(), Some("--help" | "-h" | "help")) {
        println!("{}", USAGE);
        return Ok(());
    }

    let path = argument
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

    let relay_behaviour = build_relay(&config, keypair.public().to_peer_id());

    if config.relay.enabled {
        println!(
            "relaying connections: up to {} reservation(s), {} per circuit, {} minute(s) each",
            config.relay.max_reservations,
            describe_bytes(config.relay.max_circuit_bytes),
            config.relay.max_circuit_duration_secs / 60,
        );
    } else {
        println!("not relaying connections: file transfers will not work through this server");
    }

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
                relay: relay_behaviour,
                ping: ping::Behaviour::default(),
            }
        })?
        .build();

    for address in &config.listen_on {
        swarm.listen_on(address.parse()?)?;
    }

    announce_external_addresses(&mut swarm, &config)?;

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
                // Forwarding to a client is this server's entire job, so say so
                // rather than leaving it to the mesh to work out.
                //
                // It also has to be said, because clients treat this server as
                // an explicit peer and gossipsub refuses to graft one into a
                // mesh: it answers every GRAFT with a PRUNE. Left one sided, the
                // mesh here stays empty and nothing can be forwarded directly.
                // Messages still arrive, by being advertised on the next
                // heartbeat and asked for, which turns a round trip of
                // milliseconds into one of seconds.
                swarm.behaviour_mut().gossipsub.add_explicit_peer(&peer_id);

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
                    // Or gossipsub would keep dialling somebody who has left.
                    swarm.behaviour_mut().gossipsub.remove_explicit_peer(&peer_id);

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
/// The rule for what a server address looks like is shared with the app, so that
/// an address an operator can configure here is one a person can configure
/// there, and neither end can quietly start accepting something the other will
/// not.
fn parse_siblings(addresses: &[String]) -> Result<Vec<(PeerId, Multiaddr)>, String> {
    addresses
        .iter()
        .map(|address| feed_protocol::parse_server_address(address))
        .collect()
}

/// Tells the swarm how this server is reached from outside.
///
/// Only relaying needs this, and it needs it absolutely. A reservation reply has
/// to carry the address other people will use to reach the reserving client
/// through this server, and a server listening on `0.0.0.0` has no idea what
/// that is. Without one, every reservation fails with `NoAddressesInReservation`
/// and no file transfer through this server can work.
///
/// Configured addresses are used when given. Otherwise any listen address naming
/// a real interface is used, which is right for a server bound straight to its
/// public address and wrong for one behind a container or a NAT, hence the
/// warning.
fn announce_external_addresses(
    swarm: &mut libp2p::Swarm<ServerBehaviour>,
    config: &Config,
) -> Result<(), Box<dyn Error>> {
    let announced: Vec<Multiaddr> = if config.external_addresses.is_empty() {
        config
            .listen_on
            .iter()
            .filter_map(|address| address.parse::<Multiaddr>().ok())
            .filter(|address| !is_unspecified(address))
            .collect()
    } else {
        config
            .external_addresses
            .iter()
            .map(|address| {
                address
                    .parse::<Multiaddr>()
                    .map_err(|error| format!("{} is not an address: {}", address, error))
            })
            .collect::<Result<_, _>>()?
    };

    for address in &announced {
        println!("reachable at {}", address);
        swarm.add_external_address(address.clone());
    }

    if announced.is_empty() && config.relay.enabled {
        println!(
            "WARNING: no external address is known, so clients cannot reserve a slot here and \
             file transfers through this server will fail. Set external_addresses in the \
             configuration, for example [\"/dns4/relay.example.com/tcp/4001\"]."
        );
    }

    Ok(())
}

/// Whether an address names "every interface" rather than a reachable one.
fn is_unspecified(address: &Multiaddr) -> bool {
    address.iter().any(|part| match part {
        libp2p::multiaddr::Protocol::Ip4(ip) => ip.is_unspecified(),
        libp2p::multiaddr::Protocol::Ip6(ip) => ip.is_unspecified(),
        _ => false,
    })
}

/// Builds the relay, or leaves it switched off when an operator declines to
/// carry other people's connections.
fn build_relay(config: &Config, local_peer_id: PeerId) -> Toggle<relay::Behaviour> {
    if !config.relay.enabled {
        return Toggle::from(None);
    }

    let settings = relay::Config {
        max_reservations: config.relay.max_reservations,
        max_circuit_bytes: config.relay.max_circuit_bytes,
        max_circuit_duration: Duration::from_secs(config.relay.max_circuit_duration_secs),
        ..relay::Config::default()
    };

    Toggle::from(Some(relay::Behaviour::new(local_peer_id, settings)))
}

/// A byte count as something an operator reads in a startup line.
fn describe_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return "no limit".to_string();
    }

    if bytes >= 1024 * 1024 {
        return format!("{} MiB", bytes / (1024 * 1024));
    }

    format!("{} bytes", bytes)
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
