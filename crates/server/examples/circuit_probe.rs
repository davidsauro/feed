//! Temporary check that the server relays a connection between two peers.
//!
//! Runs both ends in one process, each with its own identity and neither told
//! the other's address. The listener reserves a slot on the server, the dialler
//! reaches it through that reservation, and a stream carries bytes across. If
//! that works, a file transfer can work the same way.
//!
//!     cargo run -p indicium-server --example circuit_probe -- <server address> [megabytes]

use std::error::Error;
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{identity, noise, relay, tcp, yamux, Multiaddr, PeerId, StreamProtocol, Swarm};

const PROTOCOL: StreamProtocol = StreamProtocol::new("/circuit-probe/1.0.0");

#[derive(NetworkBehaviour)]
struct Probe {
    relay: relay::client::Behaviour,
    stream: libp2p_stream::Behaviour,
}

fn build(keypair: identity::Keypair) -> Swarm<Probe> {
    libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .expect("tcp")
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .expect("relay client")
        .with_behaviour(|_, relay| Probe {
            relay,
            stream: libp2p_stream::Behaviour::new(),
        })
        .expect("behaviour")
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let address = std::env::args().nth(1).ok_or("a server address is required")?;
    let megabytes: usize = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "1".to_string())
        .parse()?;

    let (_relay_peer, server) = indicium_protocol::parse_server_address(&address)?;

    let listener_key = identity::Keypair::generate_ed25519();
    let listener_id = PeerId::from(listener_key.public());
    let mut listener = build(listener_key);
    let mut dialler = build(identity::Keypair::generate_ed25519());

    // A reservation is what makes the listener reachable through the server.
    let reservation: Multiaddr = server.clone().with(Protocol::P2pCircuit);
    listener.listen_on(reservation.clone())?;

    let mut incoming = listener
        .behaviour()
        .stream
        .new_control()
        .accept(PROTOCOL)?;

    let payload = vec![0xABu8; megabytes * 1024 * 1024];
    let expected = payload.len();

    // The address the dialler will use, which names the server and then the
    // peer behind it. Nothing here reveals where the listener really is.
    let through_relay = reservation.with(Protocol::P2p(listener_id));

    // The reservation has to exist before anybody reaches for it, so the
    // listener is driven on its own until the server confirms one.
    let reserved_at = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match listener.select_next_some().await {
                SwarmEvent::NewListenAddr { address, .. }
                    if address.iter().any(|part| part == Protocol::P2pCircuit) =>
                {
                    return address
                }

                // The most likely thing to go wrong, and worth saying plainly.
                // A server that does not know its own public address cannot tell
                // a client where to be reached, so it refuses every reservation.
                SwarmEvent::ListenerClosed { reason, .. } => {
                    println!("RESERVATION REFUSED: {:?}", reason);
                    println!(
                        "if that mentions NoAddressesInReservation, the server needs \
                         external_addresses set in its configuration"
                    );
                    return Multiaddr::empty();
                }

                _ => {}
            }
        }
    })
    .await
    .map_err(|_| "the server never confirmed a reservation")?;

    if reserved_at.is_empty() {
        return Err("the server would not give out a reservation".into());
    }

    println!("RESERVED: {}", reserved_at);

    let served = tokio::spawn(async move {
        let (_peer, mut stream) = incoming.next().await.expect("an inbound stream");
        let mut received = Vec::new();
        stream.read_to_end(&mut received).await.expect("read");
        received.len()
    });

    let mut control = dialler.behaviour().stream.new_control();
    let dial = tokio::spawn(async move {
        let mut stream = control.open_stream(listener_id, PROTOCOL).await?;
        stream.write_all(&payload).await?;
        stream.close().await?;
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    });

    dialler.dial(DialOpts::unknown_peer_id().address(through_relay).build())?;

    let deadline = tokio::time::sleep(Duration::from_secs(90));
    tokio::pin!(deadline);
    tokio::pin!(served);
    tokio::pin!(dial);

    let mut sent = false;

    loop {
        tokio::select! {
            event = listener.select_next_some() => {
                if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event {
                    println!("listener accepted a connection from {}", peer_id);
                }
            },

            event = dialler.select_next_some() => match event {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    println!("dialler connected to {}", peer_id);
                }
                SwarmEvent::OutgoingConnectionError { error, .. } => {
                    println!("DIAL FAILED: {}", error);
                }
                _ => {}
            },

            // Guarded, or select would poll it again after it finished.
            result = &mut dial, if !sent => {
                sent = true;

                if let Err(error) = result? {
                    println!("SEND FAILED: {}", error);
                    return Err("could not send through the relay".into());
                }

                println!("sent, waiting for the far end");
            }

            received = &mut served => {
                let received = received?;
                println!("RECEIVED {} of {} bytes", received, expected);

                if received == expected {
                    println!("OK: {} MiB crossed the relay", megabytes);
                    return Ok(());
                }

                // The shape a circuit byte cap takes when it fires. The stream
                // simply ends, with nothing said about why, which is why the app
                // checks a hash rather than trusting a transfer that finished.
                println!(
                    "CUT SHORT: the relay stopped carrying at {} bytes, which usually means \
                     max_circuit_bytes",
                    received
                );

                return Err("the payload was cut short".into());
            }

            _ = &mut deadline => {
                println!("TIMED OUT");
                return Err("nothing crossed the relay in time".into());
            }
        }
    }
}
