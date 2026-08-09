# Local Mesh P2P Node

A decentralized chat application. Nodes find each other and talk directly, with
no account to create and no company in the middle. On a local network it needs
nothing else at all. To reach people elsewhere it can be pointed at one or more
relay servers, which carry traffic they cannot read.

There are two programs here:

* **The host node**, the desktop app, which is what a person runs.
* **The server**, which is optional and only needed to reach people outside your
  own network. It relays sealed traffic and knows nothing about what it carries.

## Tech Stack

* **Frontend:** Vue 3 (Composition API) + Vite
* **App shell / IPC:** Tauri 2
* **Networking:** Rust + `rust-libp2p` (gossipsub, mDNS, Noise, ping) + Tokio
* **Storage:** SQLite via `rusqlite`, optionally encrypted with SQLCipher
* **Message encryption:** X25519 + ChaCha20-Poly1305 + HKDF

## Features

* **Cryptographic identity.** An Ed25519 keypair generated on first run. The
  public half *is* the node's address on the network.
* **Local discovery** over mDNS, with no configuration.
* **Connection-based presence.** The online dot means a live connection exists.
* **Direct messages**, end-to-end encrypted, that work without either node being
  able to accept an incoming connection.
* **Group messages**, end-to-end encrypted per recipient, so relays and former
  members can read nothing.
* **File transfer**, encrypted chunk by chunk, with a whole-file hash checked on
  arrival and a partial transfer that resumes where it stopped. Local network
  only for now.
* **Self-chosen names**, offered to others as a suggestion they can override.
* **Optional encryption at rest** for the local database.
* **Optional relay servers** for reaching people beyond the local network.

---

# Host Node (Client)

## Identity

Each node generates an Ed25519 keypair on first run and keeps it in the OS
application data directory, readable only by the account that owns it. Its peer
id is derived from the public key, and for Ed25519 the key is short enough that
libp2p stores it *in* the id rather than a hash of it. Knowing somebody's peer id
therefore means holding their public key. That is what makes several of the
things below possible without any key exchange at all.

## Finding people, and knowing they are there

mDNS discovers other nodes on the same network. Discovered peers are dialled and
kept as gossipsub "explicit peers", which reconnects them automatically.

**Presence is based on connections, not on discovery.** A node that quits closes
its sockets and is marked offline at once. One that vanishes without warning,
suspended or unplugged, is noticed by `libp2p-ping` within about twenty seconds.

mDNS records are deliberately *not* used for presence, because they cannot be
trusted to expire honestly. A node's record is only refreshed when it *answers* a
query, and receiving an answer resets a node's own timer for asking. Each node's
query interval is randomised once at startup and never changes. So after the
first round every timer restarts together, and whichever node drew the shortest
interval asks first, every round, forever. That node never answers anyone, so its
record is never refreshed. Once its TTL passes it disappears from every other
node's list while being alive, connected, and perfectly reachable.

## Direct messages

This is the part that changed most, and the reasoning is worth setting out.

### How it works

Two contacts share a conversation whose name only they can compute:

```
topic = HKDF( ECDH(my private key, their public key) )
```

Diffie-Hellman is symmetric, so both sides arrive at the same name without
exchanging anything, and the name is never transmitted. Adding somebody as a
contact is the entire handshake.

Sending means sealing the message for that one person and **publishing it to that
topic** over gossipsub, rather than opening a connection to them. Their node,
which subscribes to the same topic, receives it, stores it, and publishes a small
acknowledgement back. That acknowledgement is what turns the clock beside a
message into a tick.

### Why publish instead of connect

Direct messages used to use libp2p's request-response protocol: dial the person,
send, get a reply. That works perfectly on a local network and not at all across
the internet, because most people are behind a home router that will not accept
incoming connections. Two such nodes can never reach each other, however hard
they try.

The usual answer is NAT traversal, meaning relays, hole punching, and the
machinery around them, which works most of the time and fails permanently against
some networks. Publishing to a topic sidesteps the problem instead of fighting
it, because **nobody has to accept an incoming connection.** Both nodes dial
*out* to something they can both reach, which is a thing routers have always
allowed. On a local network that something is each other. Over the internet it is
a server. The code is identical either way.

### Why a derived name, rather than an inbox per person

The obvious design gives everyone an inbox named after their peer id. Peer ids
are public, so anyone could subscribe to your inbox and collect every ciphertext
sent to you, along with who sent it and when. The content would stay sealed, but
the metadata would be wide open.

A name derived from a shared secret can only be computed by the two people
involved. A server carrying the traffic sees an opaque string.

Note what this does and does not prevent. A stranger *can* derive the name they
would use to talk to us, since that only needs our public id and their own key.
What stops them is that we subscribe only to conversations with people we have
added, so nothing they publish there is ever delivered to us.

### Why the recipient confirms delivery

"Delivered" used to mean the other node's networking accepted the bytes. With
anything in the middle that claim becomes worthless, because a server accepting a
message says nothing about whether the person ever got it.

So the recipient's own node sends the acknowledgement, after writing the message
to its database. "Delivered" now means it is on their device.

There is a cost to that honesty. If an acknowledgement is lost, a message that
really did arrive shows as failed, and nothing retries automatically. A failed
message can be clicked to send again. The id is reused, so a recipient who
already has it stores it once and simply re-acknowledges.

### Why messages carry the time they were written

When every message crossed one hop, arrival order and send order were the same
thing. Once messages can take different routes they can arrive out of order, and
a conversation that reads in the wrong order is worse than one that is slow.

Every message carries its send time *inside the sealed payload*, where nothing
carrying it can alter it, and conversations are sorted by that. A sender whose
clock is running fast is clamped, so one bad clock cannot pin a message to the
end of a conversation forever.

## Groups

A group is a topic that its members subscribe to. Messages relay through
intermediate peers, so members do not all need to be connected to each other.

**Encryption is per recipient.** Each message is encrypted once with a fresh
random key, and only that key is wrapped separately for every member. The result
is one ciphertext plus a small slot each, published once, so relaying still
works. Nobody shares a key with anybody else, which means there is no group key
to rotate and no way for someone who has left to read anything sent afterwards.
Senders simply stop making them a slot.

The alternative, one shared group key, needs a rekey every time membership
changes, and distributing that new key confidentially requires per-recipient
encryption anyway. It is this design plus a coordination problem.

Membership grows by invitation and shrinks only when somebody leaves. Nobody can
remove anybody else. That mirrors how people actually behave: a group that needs
different people in it becomes a new group. Leaving tells the others, so they
stop counting you and stop encrypting for you.

Because a message is sealed for its recipients and signed by its author, a member
cannot forge one from somebody else, replay one under their own name, or move one
between groups. What a relay can see is the group id, the sender, and roughly how
many members there are. The group's name and membership are inside the ciphertext.

Group messages report sent or failed, never read. Gossipsub can say a message
reached the mesh, not who saw it.

## Contacts, names and storage

Contacts, groups and history live in a local SQLite database. Removing a contact
deletes the conversation with them and **leaves any groups they are in**. Their
messages would be dropped on arrival, so staying would mean sitting in a
conversation silently missing everything they say. The confirmation names those
groups and counts what goes with them.

A node can choose its own name, which it announces to peers as they connect.
Others see it beside the peer id when deciding whether to add you, and it fills in
the nickname box as a starting point. The nickname each person saves is their own
and is never overwritten. **Names are claims, not identities.** Anyone can call
themselves anything, which is why the peer id stays on screen next to the name.

The database can be encrypted with a passphrase from Settings. The passphrase is
held only in memory and stored nowhere, so a forgotten one cannot be recovered.
The unlock screen's only alternative is deleting everything, which requires typing
"yes". A locked node stays off the network entirely rather than appearing online
while silently dropping messages it cannot store.

---

# Server

Optional. Everything above works on a local network with no server at all.

Full deployment, backup and operational notes: **[`crates/server/README.md`](crates/server/README.md)**

## What it is for

Reaching people who are not on your network. Two nodes behind separate home
routers cannot connect to each other. A server sits somewhere reachable, and both
of them dial out to it.

## What it does

Almost nothing, deliberately. It runs gossipsub, and it **mirrors the
subscriptions of the clients connected to it**. When a client starts listening to
a conversation, the server starts listening to the same one, which puts it in
that conversation's mesh and makes it a relay for it. When the last client
interested in a conversation disconnects, it stops.

Knowing a client is interested is not enough, because gossipsub only forwards
messages for topics a node is subscribed to itself. Mirroring is what turns
knowledge into carriage.

It learns what to mirror because gossipsub tells it. Subscriptions are announced
between peers as part of the protocol, which is how any node knows where to send
anything.

It also **relays connections**, which is a second and separate job. A
conversation is a topic, and a topic is not a connection. A file transfer needs a
real byte stream between two people, so a client asks the server for a
reservation, which makes it reachable at an address naming the server and then
the client. Somebody else dials that address and the server proxies the bytes
across. This is on by default and an operator can switch it off, keeping
conversations and refusing to carry files.

The client half of this is not written yet, so **file transfers still only work
on a local network.** The server is ready for them and nothing asks it yet.

## Why it knows nothing

Messages are sealed for their recipients before they leave the sending node, so
what passes through a server is ciphertext it has no keys for. It has no concept
of a contact, a group, or a conversation. A topic is an opaque string to it.

This is the point rather than an accident: **running a server for your friends
should not ask them to trust you.** An operator cannot read messages, cannot
forge one, and cannot alter one. Every message is signed by whoever wrote it, so
a server cannot even attribute a message to the wrong person.

A relayed connection is no different. The two ends complete their own encrypted
handshake through the circuit, so a server proxying a file transfer is moving
bytes it cannot read any more than it can read a message.

What an operator *can* see is who talks to whom. A conversation is an opaque
name, but two clients interested in the same name are two clients with something
to say to each other. Sizes and timing are visible too. Relaying adds a little to
this: a circuit names both of its ends, so an operator carrying a transfer sees
which two peers are exchanging something and how much of it, though not what.
That is inherent to routing anything at all, and it is the argument for running a
server for your own circle rather than using a stranger's.

## Why servers do not mirror each other

Servers connect to each other so that people using different ones can still talk.
But a server mirrors **only its own clients**, never its siblings. If servers
mirrored each other, each would end up subscribed to every conversation in the
federation, and every message would cross every machine.

It is unnecessary as well as expensive. A server announces its own subscriptions
to its siblings like any other peer, so they already forward it exactly what it
asked for. Ordinary gossipsub does the work.

## Identity, and why the address includes it

Clients are configured with an address like

```
/dns4/relay.example.com/tcp/4001/p2p/12D3KooW...
```

The peer id on the end is not decoration. A client refuses to talk to anything
that cannot prove it holds the matching private key, which is what stops a
hijacked hostname impersonating a server. It also means the server's identity
must survive restarts, or every client's configuration becomes wrong. That is why
the identity file lives on a mounted volume, all 68 bytes of it.

## Limits

An open server is bandwidth anyone who finds it can spend. Connection limits cap
concurrent clients and half-finished handshakes, and an allowlist can restrict it
to known peers. Left empty, which is the default, it serves anyone.

Connection limits are not enough on their own. A single connection can ask for
any number of conversations, each costing memory, so the number of conversations
one client may ask for and the number the server will carry in total are capped
separately.

Relaying is capped separately, because carrying a byte stream between two people
costs an operator real bandwidth in a way that forwarding small sealed messages
does not. The one to understand is `max_circuit_bytes`, which is a budget for a
whole relayed connection rather than a file size limit, and is explained in
[CONFIGURATION.md](crates/server/CONFIGURATION.md#relay). Running a relay for
your own circle, the sensible setting is no limit at all.

Not yet implemented: a cap on how *often* a client may publish. Doing that
properly means taking over message validation from gossipsub. A publicly exposed
server should have it.

## Why more than one

Clients connect to every server they are configured with, and duplicate messages
are discarded on arrival. There is no failover logic because there is nothing to
fail over. A server going away is a connection dropping.

It also limits the damage a hostile server can do. It cannot read or forge
anything, but it *can* drop traffic, and if a message can reach its destination
by another server then dropping it achieves nothing.

---

# Development Setup

Developed on Windows 11 with WSL2 (Ubuntu 24.04) and on macOS. The same script
handles both.

### 1. Environment Preparation

`setup.sh` detects the operating system and installs what that system needs,
then installs NVM, Node.js LTS and the Rust toolchain the same way on either:

```bash
chmod +x setup.sh
./setup.sh
source ~/.bashrc
```

On Linux that means the webview and its build dependencies. On macOS it means
the Xcode Command Line Tools, which bring the compiler and everything else the
build needs. The webview on macOS is part of the system, so there is nothing to
install for it. If the Command Line Tools are missing, the script starts Apple's
installer and asks you to run it again once that finishes.

Windows is not covered by the script. The app builds there with the MSVC
toolchain and WebView2, but note that the bundled SQLCipher compiles OpenSSL from
source, which on Windows additionally wants Perl and NASM on the path.

### 2. Install Project Dependencies

```bash
npm install
```

### 3. Run the Application

```bash
npm run tauri dev
```

> **Note for WSL2 Users:** If you see `libEGL` or `MESA` warnings, or the window
> fails to render, force software rendering:
> `LIBGL_ALWAYS_SOFTWARE=1 npm run tauri dev`
>
> **Theme detection under WSL2:** the webview is `webkit2gtk`, so CSS
> `prefers-color-scheme` reports the GTK theme inside your distro rather than the
> Windows setting. It will say "light" even when Windows is dark. Use the
> Light / Dark switch in Settings, where the choice is saved and overrides
> detection. Setting a dark GTK theme
> (`gsettings set org.gnome.desktop.interface color-scheme prefer-dark`) makes
> the "System" option work too.

## Running Multiple Instances Locally

Use `NODE_ID` and `APP_PORT` so each instance gets its own identity, its own
database, and a free port:

```bash
APP_PORT=1420 NODE_ID=1 npm run tauri dev -- --port 1420
APP_PORT=1421 NODE_ID=2 npm run tauri dev -- --port 1421
APP_PORT=1422 NODE_ID=3 npm run tauri dev -- --port 1422
```

The sidebar footer shows which instance a window belongs to.

## Running a Server

Nobody needs this repository to run one:

```bash
mkdir -p data
docker run -d --name feed-server -p 4001:4001 -v "$PWD/data:/data" \
  --restart unless-stopped ghcr.io/davidsauro/feed-server:latest
```

From a clone, with compose:

```bash
cd crates/server
mkdir -p data
docker compose up -d
docker compose logs -f
```

There is a probe that stands in for the app, for checking that a server really
does carry traffic between two nodes that know nothing about each other:

```bash
cargo run -p feed-server --example probe -- <server address> /direct/1.0.0/test
cargo run -p feed-server --example probe -- <server address> /direct/1.0.0/test "hello"
```

Relaying uses a different mechanism and has its own probe, which sends a payload
between two peers that can only have crossed the server:

```bash
cargo run -p feed-server --example circuit_probe -- <server address> 25
```

If this fails where the first probe passed, the server almost certainly needs
`external_addresses` set. One listening on `0.0.0.0`, which every container does,
cannot tell a client what address to be reached at.

## Building for other platforms

The long term aim is binaries for Windows, Linux, macOS, Android and iOS. Some
honest notes on what that involves, because it is less about this code than
about how Tauri works.

**There is no cross-compiling.** A Tauri app is built on the system it targets.
Windows needs a Windows machine for WebView2 and MSVC, macOS needs a Mac for the
SDK, and Linux needs Linux. The normal answer is a CI matrix with one runner per
platform rather than one machine producing everything.

**Mobile needs a library target that this repository no longer has.** Tauri
builds mobile apps by linking the Rust side in as a library with a
`mobile_entry_point`, which means `src-tauri` needs a `[lib]` target and its
setup living in a `run()` function that both the desktop binary and the mobile
shell can call. That scaffolding came with the Tauri template and was removed
during cleanup, since it was dead code holding a second unused `Builder`.
Restoring it is mechanical, moving the contents of `main()` into a library
function, and worth doing when mobile actually starts rather than before.

**Mobile has a harder problem than packaging.** iOS restricts multicast, so mDNS
discovery needs an entitlement that Apple grants by request, and both platforms
suspend background apps aggressively. A node that is asleep is a node that is
offline, and a peer to peer app whose peers are usually suspended is a different
design problem from the desktop one. Relay servers and store and forward matter
much more on mobile than they do on a laptop.

---

# Repository Layout

A Cargo workspace with three crates, plus the Vue frontend.

```
src/                    Vue frontend
  App.vue                 owns all state and every call into the backend
  components/             presentational only
  theme.ts, types.ts, styles.css

src-tauri/              the desktop app's Rust half
  src/main.rs             storage, Tauri commands, the network task
  src/group_crypto.rs     sealing messages, and naming conversations

crates/protocol/        what the app and the server must agree on
crates/server/          the relay: config, identity, subscription mirroring
```

`crates/protocol` is deliberately tiny, holding topic prefixes and the gossipsub
configuration and nothing else. The server needs no knowledge of messages, so if
that crate ever starts growing, it is a sign the server is learning things it
should not know.

The app and the server live in one repository because a change to how nodes talk
usually touches both, and coordinating that across repositories turns every
protocol tweak into a release.
