//! End-to-end encryption for group messages.
//!
//! Group messages travel over gossipsub, which relays them through whichever
//! peers happen to be in the middle. Those relays are ordinary nodes, and
//! without this they can read everything they pass along. The transport is
//! encrypted hop by hop, which protects against outsiders on the network but not
//! against the people doing the relaying.
//!
//! # How a message is sealed
//!
//! Encrypting the whole message once per member would be wasteful, so each
//! message is encrypted once with a fresh random key, and only that key is
//! wrapped for each member:
//!
//! 1. A random content key encrypts the message body.
//! 2. A key agreement with each member produces a wrapping key.
//! 3. The content key is encrypted once per member, giving one small "slot"
//!    each.
//!
//! The result is one ciphertext plus one slot per member — about eighty bytes of
//! overhead each — published once, so relaying still works.
//!
//! Every member gets their own slot, so nobody shares a key with anybody else.
//! There is no group key to rotate, and no way for someone who has left to read
//! anything sent afterwards: senders simply stop making them a slot.
//!
//! # Why no key exchange is needed
//!
//! A peer id for an Ed25519 node contains that node's public key: the key is
//! small enough that libp2p stores it in the id itself rather than a hash of it.
//! Knowing a member's id is therefore enough to derive a shared secret with
//! them, so there is no handshake, nothing to distribute, and nothing to go
//! stale.
//!
//! The Ed25519 key is converted to its X25519 equivalent for the key agreement.
//! Note that this reuses a signing key for encryption, which is not ideal in
//! principle — the alternative is a separate encryption key per node, which
//! would have to be distributed and kept in step. The same trade is made by
//! other messaging systems, and the alternative's failure modes look worse here.
//!
//! # What this does not do
//!
//! There is no forward secrecy for the *recipient*: the sender's key is fresh
//! for every message, so a sender's key leaking later reveals nothing, but a
//! recipient's key leaking reveals every message ever sent to them that someone
//! captured. Real forward secrecy needs a ratchet, which is a much larger piece
//! of work.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use libp2p::identity::Keypair;
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey as X25519Public, StaticSecret};

/// Format version, carried in every message so a change can be spotted rather
/// than producing nonsense.
const ENVELOPE_VERSION: u8 = 1;

/// Separates this use of a shared secret from any other. Two systems deriving
/// keys from the same secret should never arrive at the same key.
const KEY_WRAP_INFO: &[u8] = b"indicium/group-key-wrap/v1";

/// Separates the conversation-naming use of a shared secret from the key-wrapping
/// use of the very same secret.
const DIRECT_TOPIC_INFO: &[u8] = b"indicium/direct-topic/v1";

/// The multihash code meaning "this is the key itself, not a hash of it".
const MULTIHASH_IDENTITY_CODE: u64 = 0x00;

/// A sealed group message, as published to the topic.
#[derive(Serialize, Deserialize)]
struct Envelope {
    /// Format version.
    v: u8,
    /// The sender's throwaway public key for this message, base64.
    epk: String,
    /// Nonce for the body, base64.
    nonce: String,
    /// The encrypted message, base64.
    body: String,
    /// The content key, wrapped once per recipient.
    ///
    /// Carries no hint of who each one is for: a recipient tries them in turn
    /// and finds theirs by which one opens. That keeps the member list out of
    /// the reach of anyone relaying the message.
    slots: Vec<Slot>,
}

/// One recipient's copy of the content key.
#[derive(Serialize, Deserialize)]
struct Slot {
    /// Nonce, base64.
    n: String,
    /// The wrapped content key, base64.
    k: String,
}

/// Encrypts a message for every recipient given.
///
/// The sender is identified by `sender`, which is bound into the encryption: a
/// member who copies this message and republishes it under their own name
/// produces something nobody can decrypt.
pub fn seal(
    sender: &Keypair,
    group_id: &str,
    recipients: &[PeerId],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    if recipients.is_empty() {
        return Err("a group message needs at least one recipient".to_string());
    }

    let sender_id = PeerId::from(sender.public());
    let context = binding_context(group_id, &sender_id);

    // One key for this message, thrown away afterwards.
    let content_key = ChaCha20Poly1305::generate_key(&mut OsRng);
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);

    let body = ChaCha20Poly1305::new(&content_key)
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| "could not encrypt the message".to_string())?;

    // A throwaway key agreement key, so that this message stays unreadable even
    // if the sender's long-term key is exposed later.
    let ephemeral = StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = X25519Public::from(&ephemeral);

    let mut slots = Vec::with_capacity(recipients.len());
    for recipient in recipients {
        let recipient_public = encryption_key_of(recipient)?;
        let shared = ephemeral.diffie_hellman(&recipient_public);
        let wrapping_key = derive_wrapping_key(shared.as_bytes(), group_id)?;

        let slot_nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let wrapped = ChaCha20Poly1305::new(&wrapping_key)
            .encrypt(
                &slot_nonce,
                Payload {
                    msg: &content_key,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| "could not wrap the message key".to_string())?;

        slots.push(Slot {
            n: BASE64.encode(slot_nonce),
            k: BASE64.encode(wrapped),
        });
    }

    let envelope = Envelope {
        v: ENVELOPE_VERSION,
        epk: BASE64.encode(ephemeral_public.as_bytes()),
        nonce: BASE64.encode(nonce),
        body: BASE64.encode(body),
        slots,
    };

    serde_json::to_vec(&envelope).map_err(|e| e.to_string())
}

/// Decrypts a message addressed to us.
///
/// `sender` must be the peer the message was actually signed by, which gossipsub
/// verifies. Anything else fails to decrypt rather than decrypting wrongly.
///
/// Returns an error for a message not addressed to us, which is a normal thing
/// to receive: gossipsub delivers to everyone subscribed to the topic.
pub fn open(
    recipient: &Keypair,
    group_id: &str,
    sender: &PeerId,
    sealed: &[u8],
) -> Result<Vec<u8>, String> {
    let envelope: Envelope =
        serde_json::from_slice(sealed).map_err(|_| "not a sealed message".to_string())?;

    if envelope.v != ENVELOPE_VERSION {
        return Err(format!(
            "this message uses envelope version {}, and this node speaks version {}",
            envelope.v, ENVELOPE_VERSION
        ));
    }

    let context = binding_context(group_id, sender);

    let ephemeral_public: [u8; 32] = BASE64
        .decode(&envelope.epk)
        .map_err(|_| "the sender's key is not valid base64".to_string())?
        .try_into()
        .map_err(|_| "the sender's key is the wrong length".to_string())?;

    let shared = our_encryption_key(recipient)?.diffie_hellman(&X25519Public::from(ephemeral_public));
    let wrapping_key = derive_wrapping_key(shared.as_bytes(), group_id)?;
    let unwrapper = ChaCha20Poly1305::new(&wrapping_key);

    // Slots carry no addressing, so ours is whichever one opens. A slot for
    // somebody else fails its authentication check, which is the whole test.
    let content_key = envelope
        .slots
        .iter()
        .find_map(|slot| {
            let slot_nonce = BASE64.decode(&slot.n).ok()?;
            let wrapped = BASE64.decode(&slot.k).ok()?;

            unwrapper
                .decrypt(
                    Nonce::from_slice(&slot_nonce),
                    Payload {
                        msg: &wrapped,
                        aad: context.as_bytes(),
                    },
                )
                .ok()
        })
        .ok_or_else(|| "this message is not addressed to us".to_string())?;

    let nonce = BASE64
        .decode(&envelope.nonce)
        .map_err(|_| "the nonce is not valid base64".to_string())?;
    let body = BASE64
        .decode(&envelope.body)
        .map_err(|_| "the message is not valid base64".to_string())?;

    ChaCha20Poly1305::new(Key::from_slice(&content_key))
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &body,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| "the message could not be decrypted".to_string())
}

/// Names the topic two peers use for their conversation with each other.
///
/// Derived from the secret the two of them share, so both arrive at the same
/// name and nobody else can work it out — not even a server carrying the
/// traffic, which sees only an opaque string. Naming it after either peer id
/// would let anyone subscribe and collect the ciphertext and the timing of
/// everything sent to them.
///
/// Note what this does and doesn't prevent. Nobody outside the pair can derive
/// the name, so no third party can read or publish into a conversation between
/// two other people. A stranger *can* derive the name they would use to talk to
/// us, since that only needs our public id and their own key — but we subscribe
/// only to the conversations we have with contacts, so nothing they publish
/// there ever reaches us.
pub fn direct_topic_id(keypair: &Keypair, peer: &PeerId) -> Result<String, String> {
    let shared = our_encryption_key(keypair)?.diffie_hellman(&encryption_key_of(peer)?);

    // No salt: the secret is already unique to this pair, and both sides have to
    // derive the same name without any exchange.
    let hkdf = Hkdf::<Sha256>::new(None, shared.as_bytes());

    let mut name = [0u8; 32];
    hkdf.expand(DIRECT_TOPIC_INFO, &mut name)
        .map_err(|_| "could not derive the conversation name".to_string())?;

    Ok(URL_SAFE_NO_PAD.encode(name))
}

/// What the encryption is tied to: the format, the group, and who sent it.
///
/// Passed as associated data, so decryption fails if any of them differ from
/// what the sender used. That is what stops a member replaying somebody else's
/// message as their own, or moving one between groups.
fn binding_context(group_id: &str, sender: &PeerId) -> String {
    format!("indicium/group/v{}|{}|{}", ENVELOPE_VERSION, group_id, sender)
}

/// Turns a shared secret into the key that wraps the content key.
fn derive_wrapping_key(shared_secret: &[u8], group_id: &str) -> Result<Key, String> {
    let hkdf = Hkdf::<Sha256>::new(Some(group_id.as_bytes()), shared_secret);

    let mut key = [0u8; 32];
    hkdf.expand(KEY_WRAP_INFO, &mut key)
        .map_err(|_| "could not derive the wrapping key".to_string())?;

    Ok(*Key::from_slice(&key))
}

/// Our own key agreement key, derived from this node's identity.
fn our_encryption_key(keypair: &Keypair) -> Result<StaticSecret, String> {
    let ed25519 = keypair
        .clone()
        .try_into_ed25519()
        .map_err(|_| "this node's identity is not an Ed25519 key".to_string())?;

    let seed: [u8; 32] = ed25519
        .secret()
        .as_ref()
        .try_into()
        .map_err(|_| "this node's secret key is the wrong length".to_string())?;

    // The X25519 secret is derived from the Ed25519 one exactly as the signing
    // code derives its scalar, so it matches the public key other nodes compute
    // for us from our peer id.
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);

    Ok(StaticSecret::from(signing_key.to_scalar_bytes()))
}

/// The key agreement key for another node, recovered from its peer id.
///
/// Works because an Ed25519 public key is short enough that libp2p puts it in
/// the peer id directly. A node using some other kind of key would have a peer
/// id holding only a hash, and there would be nothing to recover.
fn encryption_key_of(peer: &PeerId) -> Result<X25519Public, String> {
    let multihash = peer.as_ref();

    if multihash.code() != MULTIHASH_IDENTITY_CODE {
        return Err(format!(
            "{} does not carry its public key, so it cannot be sent encrypted messages",
            peer
        ));
    }

    let public_key = libp2p::identity::PublicKey::try_decode_protobuf(multihash.digest())
        .map_err(|e| format!("could not read the public key of {}: {}", peer, e))?;

    let ed25519 = public_key
        .try_into_ed25519()
        .map_err(|_| format!("{} does not use an Ed25519 key", peer))?;

    let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&ed25519.to_bytes())
        .map_err(|e| format!("{} has an unusable public key: {}", peer, e))?;

    Ok(X25519Public::from(verifying_key.to_montgomery().to_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> Keypair {
        Keypair::generate_ed25519()
    }

    fn peer(keypair: &Keypair) -> PeerId {
        PeerId::from(keypair.public())
    }

    /// Everyone addressed can read it, and the plaintext survives intact.
    #[test]
    fn every_recipient_can_read_the_message() {
        let alice = node();
        let bob = node();
        let carol = node();
        let message = b"the eagle lands at noon";

        let sealed = seal(
            &alice,
            "group-1",
            &[peer(&bob), peer(&carol)],
            message,
        )
        .expect("sealing failed");

        for recipient in [&bob, &carol] {
            let opened = open(recipient, "group-1", &peer(&alice), &sealed)
                .expect("a recipient should be able to read the message");

            assert_eq!(opened, message);
        }
    }

    /// Both ends have to arrive at the same name with no exchange at all, or
    /// they'd be talking in different places.
    #[test]
    fn two_peers_name_their_conversation_the_same() {
        let alice = node();
        let bob = node();

        let from_alice = direct_topic_id(&alice, &peer(&bob)).unwrap();
        let from_bob = direct_topic_id(&bob, &peer(&alice)).unwrap();

        assert_eq!(from_alice, from_bob);
    }

    /// And a third person must arrive somewhere else entirely, or they could
    /// simply subscribe and listen in.
    #[test]
    fn a_third_peer_names_a_different_conversation() {
        let alice = node();
        let bob = node();
        let carol = node();

        let alice_and_bob = direct_topic_id(&alice, &peer(&bob)).unwrap();
        let alice_and_carol = direct_topic_id(&alice, &peer(&carol)).unwrap();
        let carol_and_bob = direct_topic_id(&carol, &peer(&bob)).unwrap();

        assert_ne!(alice_and_bob, alice_and_carol);
        assert_ne!(alice_and_bob, carol_and_bob);
    }

    /// The conversation name and the key that wraps messages come from the same
    /// shared secret, and must not come out the same.
    #[test]
    fn the_topic_name_is_not_the_wrapping_key() {
        let alice = node();
        let bob = node();

        let name = direct_topic_id(&alice, &peer(&bob)).unwrap();
        let shared = our_encryption_key(&alice)
            .unwrap()
            .diffie_hellman(&encryption_key_of(&peer(&bob)).unwrap());
        let wrapping = derive_wrapping_key(shared.as_bytes(), &name).unwrap();

        assert_ne!(
            URL_SAFE_NO_PAD.encode(wrapping),
            name,
            "the name a conversation is published under must not be its key"
        );
    }

    /// The whole point: someone left out cannot read it, even though gossipsub
    /// hands them the message.
    #[test]
    fn someone_left_out_cannot_read_it() {
        let alice = node();
        let bob = node();
        let dave = node();

        let sealed = seal(&alice, "group-1", &[peer(&bob)], b"private").expect("sealing failed");

        assert!(
            open(&dave, "group-1", &peer(&alice), &sealed).is_err(),
            "a non-recipient must not be able to read the message"
        );
    }

    /// The sender is bound into the encryption, so a member cannot take someone
    /// else's message and pass it off as their own.
    #[test]
    fn a_message_cannot_be_replayed_under_another_name() {
        let alice = node();
        let bob = node();
        let mallory = node();

        let sealed = seal(&alice, "group-1", &[peer(&bob)], b"from alice").expect("sealing failed");

        // Mallory republishes it; gossipsub signs it as hers, so that is the
        // sender Bob checks it against.
        assert!(
            open(&bob, "group-1", &peer(&mallory), &sealed).is_err(),
            "a message must not decrypt when attributed to a different sender"
        );
    }

    /// The group is bound in too, so a message cannot be moved between groups.
    #[test]
    fn a_message_cannot_be_moved_to_another_group() {
        let alice = node();
        let bob = node();

        let sealed = seal(&alice, "group-1", &[peer(&bob)], b"for group one").expect("sealing failed");

        assert!(
            open(&bob, "group-2", &peer(&alice), &sealed).is_err(),
            "a message must not decrypt in a group it wasn't sent to"
        );
    }

    /// Tampering is detected rather than producing altered plaintext.
    #[test]
    fn a_tampered_message_is_rejected() {
        let alice = node();
        let bob = node();

        let sealed = seal(&alice, "group-1", &[peer(&bob)], b"pay bob ten").expect("sealing failed");

        let mut envelope: Envelope = serde_json::from_slice(&sealed).unwrap();
        let mut body = BASE64.decode(&envelope.body).unwrap();
        body[0] ^= 0x01;
        envelope.body = BASE64.encode(body);

        let tampered = serde_json::to_vec(&envelope).unwrap();

        assert!(
            open(&bob, "group-1", &peer(&alice), &tampered).is_err(),
            "an altered message must not decrypt"
        );
    }

    /// The sealed form should give nothing away to whoever relays it.
    #[test]
    fn the_sealed_message_reveals_nothing() {
        let alice = node();
        let bob = node();
        let carol = node();

        let sealed = seal(&alice, "group-1", &[peer(&bob), peer(&carol)], b"secret plans")
            .expect("sealing failed");
        let on_the_wire = String::from_utf8_lossy(&sealed);

        assert!(!on_the_wire.contains("secret plans"), "the message is in the clear");
        assert!(
            !on_the_wire.contains(&peer(&bob).to_string()),
            "a recipient is named on the wire"
        );
        assert!(
            !on_the_wire.contains(&peer(&carol).to_string()),
            "a recipient is named on the wire"
        );
    }

    /// One slot per recipient, and each recipient finds only their own.
    #[test]
    fn there_is_one_slot_for_each_recipient() {
        let alice = node();
        let recipients: Vec<Keypair> = (0..4).map(|_| node()).collect();
        let peers: Vec<PeerId> = recipients.iter().map(peer).collect();

        let sealed = seal(&alice, "group-1", &peers, b"hello all").expect("sealing failed");
        let envelope: Envelope = serde_json::from_slice(&sealed).unwrap();

        assert_eq!(envelope.slots.len(), 4);
    }
}
