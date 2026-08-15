//! Measures how long a message takes to cross a server.
//!
//! Two peers connect to the same server and to nothing else, one publishes, and
//! the other reports how long it waited. Both add the server as an explicit peer
//! exactly as the app does, so what this measures is the path a real message
//! takes rather than an idealised one.
//!
//!     cargo run -p indicium-server --example latency_probe -- <server address> [count]

use std::error::Error;
use std::time::{Duration, Instant};

use futures::stream::StreamExt;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identity, noise, tcp, yamux, PeerId, Swarm};

#[derive(NetworkBehaviour)]
struct Probe {
    gossipsub: gossipsub::Behaviour,
}

fn build(relay: PeerId) -> Swarm<Probe> {
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(identity::Keypair::generate_ed25519())
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .expect("tcp")
        .with_behaviour(|key| Probe {
            gossipsub: gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                indicium_protocol::gossipsub_config().expect("config"),
            )
            .expect("gossipsub"),
        })
        .expect("behaviour")
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    // What the app does, and the thing that makes the server unable to graft us.
    swarm.behaviour_mut().gossipsub.add_explicit_peer(&relay);
    swarm
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::args().nth(1).ok_or("a server address is required")?;
    let count: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "5".to_string())
        .parse()?;

    let (relay, server) = indicium_protocol::parse_server_address(&address)?;

    let topic = gossipsub::IdentTopic::new(format!("{}latency-probe", indicium_protocol::DIRECT_TOPIC_PREFIX));

    let mut sender = build(relay);
    let mut receiver = build(relay);

    sender.behaviour_mut().gossipsub.subscribe(&topic)?;
    receiver.behaviour_mut().gossipsub.subscribe(&topic)?;

    sender.dial(server.clone())?;
    receiver.dial(server)?;

    // Let the connections and subscriptions settle before timing anything.
    let settle = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(settle);

    loop {
        tokio::select! {
            _ = sender.select_next_some() => {}
            _ = receiver.select_next_some() => {}
            _ = &mut settle => break,
        }
    }

    let mut times = Vec::new();

    for round in 1..=count {
        let sent = Instant::now();

        if let Err(error) = sender
            .behaviour_mut()
            .gossipsub
            .publish(topic.clone(), format!("round {}", round).into_bytes())
        {
            println!("could not publish: {}", error);
            return Err("nobody was listening".into());
        }

        let deadline = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(deadline);

        let arrived = loop {
            tokio::select! {
                _ = sender.select_next_some() => {}

                event = receiver.select_next_some() => {
                    if let SwarmEvent::Behaviour(ProbeEvent::Gossipsub(
                        gossipsub::Event::Message { .. },
                    )) = event
                    {
                        break Some(sent.elapsed());
                    }
                }

                _ = &mut deadline => break None,
            }
        };

        match arrived {
            Some(elapsed) => {
                println!("round {}: {} ms", round, elapsed.as_millis());
                times.push(elapsed);
            }
            None => println!("round {}: never arrived", round),
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if times.is_empty() {
        return Err("nothing arrived".into());
    }

    let total: Duration = times.iter().sum();
    println!(
        "\nAVERAGE over {} message(s): {} ms",
        times.len(),
        total.as_millis() / times.len() as u128
    );

    Ok(())
}
