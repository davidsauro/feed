//! A stand-in for the app, for checking that a server carries traffic.
//!
//! Two of these connect to the same server and to nothing else — they are never
//! told each other's addresses and have no way to find one another. If a message
//! published by one arrives at the other, the only path it can have taken is
//! through the server, which is the whole premise the design rests on.
//!
//!     cargo run -p feed-server --example probe -- <server address> <topic>
//!     cargo run -p feed-server --example probe -- <server address> <topic> "hello"
//!
//! Without a message it listens and prints what arrives. With one it waits for
//! the server to start carrying the conversation, then publishes.
//!
//! The payload here is plain text. A real client seals it before it leaves, so
//! what the server carries is ciphertext; none of that matters to the server, so
//! none of it is needed to test the server.

use std::error::Error;
use std::time::Duration;

use futures::stream::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identity, noise, tcp, yamux, Multiaddr};

#[derive(NetworkBehaviour)]
struct ProbeBehaviour {
    gossipsub: gossipsub::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);

    let server: Multiaddr = args
        .next()
        .ok_or("usage: probe <server address> <topic> [message]")?
        .parse()?;
    let topic_name = args.next().ok_or("a topic is required")?;
    let message = args.next();

    let server_peer = server
        .iter()
        .find_map(|part| match part {
            Protocol::P2p(peer) => Some(peer),
            _ => None,
        })
        .ok_or("the server address must end with /p2p/<peer id>")?;

    let keypair = identity::Keypair::generate_ed25519();
    println!("probe is {}", keypair.public().to_peer_id());

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            // The same configuration the app and the server use. A probe that
            // configured gossipsub differently would prove nothing about them.
            let config = feed_protocol::gossipsub_config().expect("shared config is not valid");

            ProbeBehaviour {
                gossipsub: gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    config,
                )
                .expect("failed to start gossipsub"),
            }
        })?
        .build();

    let topic = gossipsub::IdentTopic::new(&topic_name);
    swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
    swarm.behaviour_mut().gossipsub.add_explicit_peer(&server_peer);
    swarm.dial(server.clone())?;

    println!("dialling {}", server);

    let mut published = false;

    loop {
        let event = tokio::time::timeout(Duration::from_secs(20), swarm.select_next_some()).await;

        let Ok(event) = event else {
            println!("nothing happened for 20 seconds, giving up");
            return Ok(());
        };

        match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("connected to {}", peer_id);
            }

            // The server announcing it is carrying our conversation. That is the
            // moment publishing can work, so it beats guessing with a sleep.
            SwarmEvent::Behaviour(ProbeBehaviourEvent::Gossipsub(
                gossipsub::Event::Subscribed { peer_id, topic: carried },
            )) => {
                println!("{} is carrying {}", peer_id, carried);

                if let Some(text) = &message {
                    if !published {
                        match swarm.behaviour_mut().gossipsub.publish(topic.clone(), text.as_bytes())
                        {
                            Ok(_) => println!("published: {}", text),
                            Err(error) => println!("could not publish: {}", error),
                        }
                        published = true;
                    }
                }
            }

            SwarmEvent::Behaviour(ProbeBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                message,
                ..
            })) => {
                println!(
                    "RECEIVED from {}: {}",
                    message
                        .source
                        .map(|peer| peer.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                    String::from_utf8_lossy(&message.data)
                );
            }

            _ => {}
        }
    }
}
