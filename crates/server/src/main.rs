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
//! your friends does not ask them to trust you.
//!
//! What it will do, once finished:
//!
//! - listen on a stable address with an identity that survives restarts, since
//!   the identity is half of the address clients are configured with
//! - carry the conversations its own clients ask for, and no others
//! - refuse anyone not on the allowlist, when the operator has set one
//! - hold connections and subscriptions within limits an open server can afford
//! - connect to sibling servers, so people configured with different ones can
//!   still reach each other
//!
//! None of that is here yet. This is the skeleton: it builds, it listens, and it
//! reports what connects to it.

use std::error::Error;

use futures::stream::StreamExt;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identity, noise, tcp, yamux};

/// Where to listen. Configurable once there is configuration to speak of.
const LISTEN_ADDRESS: &str = "/ip4/0.0.0.0/tcp/4001";

/// Everything this server does on the network.
#[derive(NetworkBehaviour)]
struct ServerBehaviour {
    /// The one protocol that matters here: it carries every message this server
    /// passes along, for topics it has been asked to carry.
    gossipsub: gossipsub::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // A throwaway identity for now. It has to become a key loaded from disk
    // before this is useful to anyone: clients address a server by its public
    // key as much as by its hostname, so an identity that changes on restart
    // would break every client's configuration.
    let keypair = identity::Keypair::generate_ed25519();
    println!("this server is {}", keypair.public().to_peer_id());

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
            let config = feed_protocol::gossipsub_config()
                .expect("the shared gossipsub configuration is not valid");

            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                config,
            )
            .expect("failed to start gossipsub");

            ServerBehaviour { gossipsub }
        })?
        .build();

    swarm.listen_on(LISTEN_ADDRESS.parse()?)?;

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("listening on {}", address);
            }

            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("{} connected", peer_id);
            }

            SwarmEvent::ConnectionClosed { peer_id, .. } => {
                println!("{} disconnected", peer_id);
            }

            // Carrying these is the next piece of work. Nothing is subscribed
            // yet, so nothing arrives yet.
            SwarmEvent::Behaviour(ServerBehaviourEvent::Gossipsub(event)) => {
                println!("gossipsub: {:?}", event);
            }

            _ => {}
        }
    }
}
