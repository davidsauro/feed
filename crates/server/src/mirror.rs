//! Deciding which conversations this server carries.
//!
//! Gossipsub only forwards messages for topics a node is subscribed to itself.
//! Knowing that a client is interested in a conversation is therefore not enough
//! to carry it — the server has to subscribe to the same topic, which puts it in
//! that topic's mesh and makes it a relay.
//!
//! So this keeps a count: which clients want which conversations. The first
//! client to ask makes the server subscribe; the last one to leave makes it
//! unsubscribe. Without that count a server would accumulate every conversation
//! it had ever been asked about and hold mesh state for people who left hours
//! ago.
//!
//! Only clients are counted here. Mirroring another server's subscriptions would
//! make every server in a federation carry every conversation in it, because
//! each would mirror the other's mirroring. It isn't needed either: a server
//! announces its own subscriptions to its siblings like any peer, so they
//! already forward it what it asked for.
//!
//! Nothing in here can read anything. A topic is an opaque name, and what
//! travels under it is sealed for people this server has no keys for.

use std::collections::{HashMap, HashSet};

use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;

/// What the server should do about a client's request to carry a conversation.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Subscribe: this is the first client to ask for it.
    Subscribe,
    /// Already carried for somebody else; nothing to do on the network.
    AlreadyCarried,
    /// Declined, with a reason worth logging.
    Refused(Refusal),
}

/// Why a request to carry a conversation was turned down.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a topic this application uses. Carrying it would make this server a
    /// relay for whatever else is on the network.
    NotOurs,
    /// This client is already having as many conversations as it is allowed.
    PeerLimit,
    /// The server is carrying as many conversations as it is allowed.
    ServerLimit,
}

/// How much one server, and one client, is allowed to take on.
pub struct Caps {
    pub per_peer: usize,
    pub total: usize,
}

pub struct Mirror {
    /// Who wants each conversation. A conversation with nobody left wanting it
    /// is dropped.
    interested: HashMap<TopicHash, HashSet<PeerId>>,
    /// The same relationship the other way round, so that a client disconnecting
    /// doesn't mean searching every conversation.
    wanted_by: HashMap<PeerId, HashSet<TopicHash>>,
    caps: Caps,
}

impl Mirror {
    pub fn new(caps: Caps) -> Self {
        Self {
            interested: HashMap::new(),
            wanted_by: HashMap::new(),
            caps,
        }
    }

    /// Records that a client wants a conversation carried.
    pub fn wanted(&mut self, peer: PeerId, topic: TopicHash) -> Decision {
        if !indicium_protocol::is_app_topic(topic.as_str()) {
            return Decision::Refused(Refusal::NotOurs);
        }

        let already = self.wanted_by.get(&peer).is_some_and(|t| t.contains(&topic));
        if already {
            return Decision::AlreadyCarried;
        }

        let peer_count = self.wanted_by.get(&peer).map_or(0, |t| t.len());
        if peer_count >= self.caps.per_peer {
            return Decision::Refused(Refusal::PeerLimit);
        }

        let new_to_us = !self.interested.contains_key(&topic);
        if new_to_us && self.interested.len() >= self.caps.total {
            return Decision::Refused(Refusal::ServerLimit);
        }

        self.interested.entry(topic.clone()).or_default().insert(peer);
        self.wanted_by.entry(peer).or_default().insert(topic);

        if new_to_us {
            Decision::Subscribe
        } else {
            Decision::AlreadyCarried
        }
    }

    /// Records that a client no longer wants a conversation.
    ///
    /// Returns the topic if that was the last client interested, meaning the
    /// server should stop carrying it.
    pub fn no_longer_wanted(&mut self, peer: &PeerId, topic: &TopicHash) -> Option<TopicHash> {
        if let Some(topics) = self.wanted_by.get_mut(peer) {
            topics.remove(topic);

            if topics.is_empty() {
                self.wanted_by.remove(peer);
            }
        }

        let peers = self.interested.get_mut(topic)?;
        peers.remove(peer);

        if peers.is_empty() {
            self.interested.remove(topic);
            return Some(topic.clone());
        }

        None
    }

    /// Forgets a client entirely, on disconnection.
    ///
    /// Returns the conversations nobody is left wanting.
    pub fn forget(&mut self, peer: &PeerId) -> Vec<TopicHash> {
        let Some(topics) = self.wanted_by.remove(peer) else {
            return Vec::new();
        };

        let mut dropped = Vec::new();

        for topic in topics {
            if let Some(peers) = self.interested.get_mut(&topic) {
                peers.remove(peer);

                if peers.is_empty() {
                    self.interested.remove(&topic);
                    dropped.push(topic);
                }
            }
        }

        dropped
    }

    /// How many conversations this server is carrying.
    pub fn carried(&self) -> usize {
        self.interested.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> Caps {
        Caps {
            per_peer: 3,
            total: 5,
        }
    }

    fn topic(name: &str) -> TopicHash {
        TopicHash::from_raw(format!("/direct/1.0.0/{}", name))
    }

    /// The first client to ask makes the server subscribe; later ones don't.
    #[test]
    fn subscribes_once_however_many_people_want_it() {
        let mut mirror = Mirror::new(caps());
        let (alice, bob) = (PeerId::random(), PeerId::random());

        assert_eq!(mirror.wanted(alice, topic("a")), Decision::Subscribe);
        assert_eq!(mirror.wanted(bob, topic("a")), Decision::AlreadyCarried);
        assert_eq!(mirror.carried(), 1);
    }

    /// And it keeps carrying it until the last of them is gone.
    #[test]
    fn stops_carrying_when_the_last_client_leaves() {
        let mut mirror = Mirror::new(caps());
        let (alice, bob) = (PeerId::random(), PeerId::random());

        mirror.wanted(alice, topic("a"));
        mirror.wanted(bob, topic("a"));

        assert_eq!(mirror.no_longer_wanted(&alice, &topic("a")), None);
        assert_eq!(
            mirror.no_longer_wanted(&bob, &topic("a")),
            Some(topic("a")),
            "the last one leaving should end it"
        );
        assert_eq!(mirror.carried(), 0);
    }

    /// A client that vanishes takes its conversations with it, which is the
    /// usual way one ends.
    #[test]
    fn disconnecting_gives_up_everything_that_client_wanted() {
        let mut mirror = Mirror::new(caps());
        let (alice, bob) = (PeerId::random(), PeerId::random());

        mirror.wanted(alice, topic("a"));
        mirror.wanted(alice, topic("b"));
        mirror.wanted(bob, topic("b"));

        let dropped = mirror.forget(&alice);

        assert_eq!(dropped, vec![topic("a")], "b is still wanted by bob");
        assert_eq!(mirror.carried(), 1);
    }

    /// One client cannot make the server carry an unlimited number of
    /// conversations.
    #[test]
    fn one_client_is_capped() {
        let mut mirror = Mirror::new(caps());
        let alice = PeerId::random();

        for name in ["a", "b", "c"] {
            assert_eq!(mirror.wanted(alice, topic(name)), Decision::Subscribe);
        }

        assert_eq!(
            mirror.wanted(alice, topic("d")),
            Decision::Refused(Refusal::PeerLimit)
        );
    }

    /// Nor can clients between them.
    #[test]
    fn the_server_as_a_whole_is_capped() {
        let mut mirror = Mirror::new(caps());

        for name in ["a", "b", "c", "d", "e"] {
            mirror.wanted(PeerId::random(), topic(name));
        }

        assert_eq!(
            mirror.wanted(PeerId::random(), topic("f")),
            Decision::Refused(Refusal::ServerLimit)
        );
    }

    /// Room freed by one client is available to the next.
    #[test]
    fn leaving_frees_room_for_someone_else() {
        let mut mirror = Mirror::new(caps());
        let alice = PeerId::random();

        for name in ["a", "b", "c"] {
            mirror.wanted(alice, topic(name));
        }
        mirror.no_longer_wanted(&alice, &topic("a"));

        assert_eq!(mirror.wanted(alice, topic("d")), Decision::Subscribe);
    }

    /// This server carries this application's traffic, not anybody's.
    #[test]
    fn refuses_topics_that_are_nothing_to_do_with_us() {
        let mut mirror = Mirror::new(caps());

        assert_eq!(
            mirror.wanted(PeerId::random(), TopicHash::from_raw("/someone-else/1.0.0/x")),
            Decision::Refused(Refusal::NotOurs)
        );
        assert_eq!(mirror.carried(), 0);
    }

    /// Asking twice for the same conversation must not consume two places
    /// against the cap.
    #[test]
    fn asking_twice_costs_nothing() {
        let mut mirror = Mirror::new(caps());
        let alice = PeerId::random();

        mirror.wanted(alice, topic("a"));
        assert_eq!(mirror.wanted(alice, topic("a")), Decision::AlreadyCarried);

        for name in ["b", "c"] {
            assert_eq!(mirror.wanted(alice, topic(name)), Decision::Subscribe);
        }
    }
}
