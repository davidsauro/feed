//! Checks that a reservation survives the app's ordering of events.
//!
//! The app dials a server first and asks to be reachable through it second,
//! which is not what the simpler probes do. That order matters: the relay client
//! asks for a reservation over an existing connection when it finds one, and
//! dials the relay itself when it does not. A dial issued while one is already
//! in flight is refused, taking the reservation with it.
//!
//!     cargo run -p feed-server --example reserve_probe -- <server address> [when]
//!
//! `when` is `after` (the default, what the app does now) or `during`, which
//! reproduces the failure.

use std::error::Error;
use std::time::Duration;

use futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::SwarmEvent;
use libp2p::{identity, noise, ping, relay, tcp, yamux, Multiaddr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::args().nth(1).ok_or("a server address is required")?;
    let when = std::env::args().nth(2).unwrap_or_else(|| "after".to_string());

    let (relay_peer, server) = feed_protocol::parse_server_address(&address)?;

    let mut swarm =
        libp2p::SwarmBuilder::with_existing_identity(identity::Keypair::generate_ed25519())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|_, relay| Behaviour {
                relay,
                ping: ping::Behaviour::default(),
            })?
            .build();

    let circuit: Multiaddr = server.clone().with(Protocol::P2pCircuit);

    // What the app does: its own dial, exactly as `dial_server` builds it.
    swarm.dial(DialOpts::peer_id(relay_peer).addresses(vec![server]).build())?;

    if when == "during" {
        println!("asking for a reservation while the dial is still in flight");
        swarm.listen_on(circuit.clone())?;
    } else {
        println!("waiting for the connection before asking for a reservation");
    }

    let deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(deadline);

    let mut asked = when == "during";

    loop {
        tokio::select! {
            event = swarm.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == relay_peer => {
                    println!("connected to the server");

                    if !asked {
                        asked = true;
                        swarm.listen_on(circuit.clone())?;
                    }
                }

                SwarmEvent::NewListenAddr { address, .. }
                    if address.iter().any(|p| p == Protocol::P2pCircuit) =>
                {
                    println!("RESERVED: {}", address);
                    return Ok(());
                }

                SwarmEvent::ListenerClosed { reason, .. } => {
                    println!("RESERVATION REFUSED: {:?}", reason);
                    return Err("no reservation".into());
                }

                _ => {}
            },

            _ = &mut deadline => {
                println!("TIMED OUT with no reservation");
                return Err("no reservation".into());
            }
        }
    }
}

#[derive(libp2p::swarm::NetworkBehaviour)]
struct Behaviour {
    relay: relay::client::Behaviour,
    ping: ping::Behaviour,
}
