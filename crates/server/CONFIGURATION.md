# Configuring feed-server

Everything here is optional. A server started with no configuration at all
listens on port 4001 and carries traffic for anyone, which is a useful thing to
run as it stands. This document covers what the server does, and how to change
each part of it.

For running the thing in the first place, see [README.md](README.md).

## How configuration is loaded

The server reads `feed-server.toml` from its working directory. In the container
that is `/data`, which is the mounted volume, so the file belongs at
`data/feed-server.toml` alongside the identity.

A different path can be given as the first argument:

```bash
feed-server /etc/feed/relay.toml
```

Three rules govern what happens next:

| Situation | What happens |
|---|---|
| No file | Defaults are used, and the server says so on startup |
| File that cannot be parsed | The server stops |
| File with a key it does not recognise | The server stops |

A missing file means "run with the defaults". A file that exists but is wrong is
an operator who meant something, and guessing at it would be worse than
stopping. That includes a misspelled key, which is refused rather than silently
ignored, because a setting that looks applied but is not is the worst outcome of
the three.

Configuration is read once at startup. Changing it means restarting the server.

## What the server does

### Carries conversations for the clients connected to it

This is the whole job. A client subscribes to a conversation, the server notices
and subscribes to the same one, and from then on messages published by anyone in
that conversation reach everyone else in it through this server.

The important part is what the server subscribes **from**. It mirrors the
subscriptions of clients that connect to it, and nothing else. It does not
subscribe to what it sees on the wider network, so it never becomes a relay for
conversations nobody here asked for.

A conversation with no interested client left is dropped, so a server that has
been idle overnight is carrying nothing.

### Refuses conversations that are not this application's

A client can only ask for a conversation whose name starts with `/group/` or
`/direct/`. Anything else is declined and logged. Without that check, a server
would relay whatever else happened to be on the network for whoever asked.

### Keeps an identity between restarts

The server generates an Ed25519 key on first start and reuses it forever after.
Clients address a server by that public key as well as by its hostname, which is
what makes an address unambiguous. See
[what it stores](README.md#what-it-stores-and-how-to-back-it-up).

### Carries connections, so that file transfers work

A conversation is a topic, and a topic is not a connection. A file transfer needs
a real byte stream between two people, which is a different thing from
forwarding sealed messages, and is why relaying is a separate part of the server
rather than something gossipsub gives for free.

A client asks this server for a **reservation**, which makes it reachable at an
address naming this server and then the client. Somebody else dials that
address, the server proxies the connection, and the bytes cross. The server can
no more read a circuit than it can read a message.

This is on by default and is governed by [its own settings](#relay), separately
from the conversation limits, because it is a different kind of cost. Forwarding
sealed messages for topics people asked for is cheap and bounded. Proxying a
byte stream is bandwidth an operator spends on somebody else's file.

### Answers pings

The server responds to libp2p pings so that clients can measure the round trip
to it and show whether it is genuinely healthy rather than merely still
accepting sockets. Nothing on the server needs this. It exists for the other
end, and there is no setting for it.

### Cannot read anything it carries

Messages are sealed for their recipients before they leave the sending node.
There is no setting for this either, in the sense that no configuration can turn
it off. See
[what an operator can and cannot see](README.md#what-an-operator-can-and-cannot-see)
for what remains visible, which is not nothing.

## Settings

### `listen_on`

```toml
listen_on = ["/ip4/0.0.0.0/tcp/4001"]
```

**Default:** `["/ip4/0.0.0.0/tcp/4001"]`, meaning every network interface on port
4001.

Addresses the server accepts connections on, in multiaddress form. More than one
is allowed, which is how you listen on IPv4 and IPv6 at once:

```toml
listen_on = ["/ip4/0.0.0.0/tcp/4001", "/ip6/::/tcp/4001"]
```

**When to change it:** to use a different port, or to bind to one interface
rather than all of them.

**What to watch out for:** the port is part of the address every client is
configured with, so changing it later means updating every client. Inside Docker
this is the port *inside* the container, which is the right-hand number in the
compose file's port mapping. Changing one without the other means a server
listening where nothing is published.

### `external_addresses`

```toml
external_addresses = ["/dns4/relay.example.com/tcp/4001"]
```

**Default:** empty, in which case any `listen_on` address that names a real
interface is used.

How this server is reached from outside, without the `/p2p/` part.

**When to change it:** whenever the server is behind anything. A container
publishing a port, a NAT, or a DNS name all mean the listen address is not how
people get here.

**What to watch out for:** this is only used for relaying, and relaying does not
work at all without it. When a client reserves a slot, the reply has to say what
address others should use to reach that client through this server. Listening on
`0.0.0.0` cannot answer that, so every reservation fails with
`NoAddressesInReservation` and no file transfer through this server can work.
Conversations carry on working perfectly, which is what makes this easy to miss.

**In Docker this is effectively required**, because the address inside the
container is never the address people dial. The server prints
`reachable at <address>` for each one at startup, and warns when it has none and
relaying is on.

### `identity_file`

```toml
identity_file = "identity.bin"
```

**Default:** `identity.bin`, relative to the working directory.

Where the server's private key lives. It is created on first start with
owner-only permissions and read on every start after that.

**When to change it:** rarely. Running two servers on one host is the usual
reason, and each needs its own.

**What to watch out for:** this file **is** the server's identity. Lose it and
every client configuration pointing at this server becomes wrong, because the
peer id they hold no longer matches. Copy it and whoever holds the copy can be
this server. In a container it has to sit on the mounted volume, or a restart
means a new identity and a server nobody can reach any more.

### `allowed_peers`

```toml
allowed_peers = [
    "12D3KooWCfRCgiQUEL6MkbU8rBNGySLjJCi3iYSKRpAc2w7u8TPW",
]
```

**Default:** empty, meaning anyone may connect.

The peer ids permitted to connect. A person finds their own at the top of the
app's sidebar, next to their name, where clicking it copies the full value.

**When to change it:** whenever the server is for a known group of people rather
than for the public. An open server is bandwidth anyone who finds it can spend,
and the [limits](#limits) are all that stands between it and somebody who wants
to take advantage of that.

**What to watch out for:** blank means open, so this is a list where removing the
last entry quietly reverses the policy. Sibling servers are added to the list
automatically, so closing a server does not accidentally cut it off from the
others.

An allowlist is applied after the handshake rather than before it, because a
peer's identity is not known until the handshake proves it. There is no earlier
moment at which there would be anything to check. Refused peers therefore still
cost a handshake, which is what `max_pending_incoming` is for.

### `siblings`

```toml
siblings = [
    "/dns4/relay2.example.com/tcp/4001/p2p/12D3KooWCfRCgiQUEL6MkbU8rBNGySLjJCi3iYSKRpAc2w7u8TPW",
]
```

**Default:** empty.

Other servers to connect to. People using those servers and people using this one
can then reach each other.

**When to change it:** when running more than one relay, whether for redundancy
or because two groups want to talk without everyone moving to one server.

**What to watch out for:** each address must end with `/p2p/<peer id>`. An
address without one is refused at startup rather than dialled, because
connecting to whatever answers at a hostname is exactly what including the
identity prevents. This is the same rule the app applies to the addresses a
person configures, and both ends share the code that enforces it.

Federation across siblings is still a work in progress. The connection is made,
but treat carrying traffic between servers as untested.

## Limits

Each of these caps what the server, or one client of it, is allowed to cost.
They live in a `[limits]` section:

```toml
[limits]
max_topics_per_peer = 64
max_topics_total = 4096
max_connections_per_peer = 2
max_pending_incoming = 64
max_pending_outgoing = 16
max_connections_incoming = 512
max_connections_outgoing = 32
max_connections_total = 550
```

| Setting | Default | What it caps |
|---|---|---|
| `max_topics_per_peer` | 64 | Conversations one client may ask this server to carry |
| `max_topics_total` | 4096 | Conversations carried across the whole server |
| `max_connections_per_peer` | 2 | Connections held by any one peer |
| `max_pending_incoming` | 64 | Inbound handshakes in progress |
| `max_pending_outgoing` | 16 | Outbound handshakes in progress |
| `max_connections_incoming` | 512 | Established inbound connections, roughly the number of clients |
| `max_connections_outgoing` | 32 | Established outbound connections, which means siblings |
| `max_connections_total` | 550 | Established connections of either kind |

### The two topic limits

These are the only limits libp2p does not handle. Connection limits count
connections, and a single connection can ask for any number of conversations,
each of which costs memory here. Without these, one client could exhaust a
server without opening a second socket.

A client at its limit is declined and the refusal is logged. Nothing is dropped
for anyone else.

**Raising them:** a person in a lot of groups uses one conversation per group
plus one per contact, so 64 covers an ordinary user comfortably. Raise
`max_topics_total` before `max_topics_per_peer` if a busy server starts
declining, since the total is the one that binds first when many people share a
server.

### The connection limits

`max_pending_incoming` matters most on an open server, since handshakes that
never complete are the cheapest thing to flood one with.

`max_connections_per_peer` is 2 rather than 1 so that a new connection can be
established before an old one has finished closing. Setting it to 1 will cause
reconnections to fail intermittently for reasons that look like a network fault.

`max_connections_total` is checked alongside the two it overlaps, so whichever
is reached first is the one that bites. The defaults sit just above their sum,
512 plus 32 being 544 against a total of 550, which means in practice the
incoming limit is what stops a busy server and the total is a backstop. Setting
the total *below* the sum inverts that, and is a reasonable thing to do if you
care about the combined figure rather than about either side of it.

## Relay

Whether this server carries connections between two people who cannot reach each
other directly. Settings live in a `[relay]` section:

```toml
[relay]
enabled = true
max_circuit_bytes = 268435456
max_circuit_duration_secs = 3600
max_reservations = 512
```

| Setting | Default | What it does |
|---|---|---|
| `enabled` | `true` | Whether to relay connections at all |
| `max_circuit_bytes` | 256 MiB | Total bytes one relayed connection may carry |
| `max_circuit_duration_secs` | 3600 | How long one relayed connection may live |
| `max_reservations` | 512 | How many clients may be reachable through this server |

**Turning `enabled` off** leaves conversations working and stops file transfers
through this server. That is the switch for an operator willing to carry
messages but not files.

### `max_circuit_bytes` is not a file size limit

This is the setting people get wrong, so it is worth being blunt. A relayed
connection carries **everything** between two people, in **both directions**, for
**as long as it lives**: several transfers one after another, plus the
conversation traffic alongside them. One counter covers the lot and it never
resets.

Set it to 25 MB expecting it to cap a 25 MB file and the second file that person
sends will be cut off partway through.

Keep it well clear of ordinary use. It is a backstop against somebody streaming
without end. The app's own per-file limit is what tells a person that a
particular file is too big, and that is the one that produces a sensible message.

When this does fire, the connection simply ends. Nothing is said about why, and
the transfer is caught by its hash rather than by an error. Setting it to `0`
removes the limit.

### The other two

`max_circuit_duration_secs` is a hard timer from establishment, not an idle
timeout. The libp2p default is two minutes, which is far too short, because a
relayed connection carries the conversation as well as any transfers and should
last as long as two people are talking.

`max_reservations` wants to be in step with `max_connections_incoming`, since
every client that wants to be reachable needs one. The libp2p default of 128 sits
well below the connection limits and would leave most clients unreachable on a
busy server.

### Checking it works

```bash
cargo run -p feed-server --example circuit_probe -- <server address> 25
```

Two peers in one process, neither told the other's address, exchanging 25 MiB
that can only have crossed the relay. It reports a refused reservation and a
circuit cut short by name, which are the two ways this goes wrong.

## Recipes

### An open relay, for anyone

```toml
# Everything else can stay at its default. This one cannot be guessed, and
# without it file transfers through this server will not work.
external_addresses = ["/dns4/relay.example.com/tcp/4001"]
```

Reasonable to run if you are willing to spend the bandwidth. Read
[Limits](README.md#limits) first, and note what is not yet implemented.

### A private relay, for a known group

```toml
# Required for file transfers whenever this is not the address people dial,
# which includes every container.
external_addresses = ["/dns4/relay.example.com/tcp/4001"]

allowed_peers = [
    "12D3KooW...",  # you
    "12D3KooW...",  # a friend
]

[limits]
# A handful of known people cannot need thousands of conversations, and a lower
# cap turns a runaway client into a logged refusal rather than a memory problem.
max_topics_total = 512
max_connections_incoming = 32
max_connections_total = 64

[relay]
# No limit, which is the right answer when the operator and the users are the
# same people. The caps exist to protect an operator from strangers, and there
# are none here. With a budget set, a long session of sending files is
# interrupted every time it is reached, which is a nuisance and nothing more,
# but a pointless one among people who trust each other.
max_circuit_bytes = 0

# A day. There is no way to switch the timer off, so this is how you say
# "effectively never".
max_circuit_duration_secs = 86400
```

Everyone on the list needs the server's address. Everyone not on it is refused
after the handshake.

### Two servers, federated

On the first:

```toml
siblings = ["/dns4/relay2.example.com/tcp/4001/p2p/<second server's peer id>"]
```

On the second:

```toml
siblings = ["/dns4/relay1.example.com/tcp/4001/p2p/<first server's peer id>"]
```

Each server's peer id is printed on its first start. Siblings allowlist each
other automatically, so this works whether or not either server is otherwise
closed.

## Not settings

Things people reasonably look for and will not find.

**Rate limiting.** There is no cap on how *often* a client may publish. Doing it
properly means taking over message validation from gossipsub, which is a larger
change than it sounds. A server exposed publicly should have this and does not
yet.

**Store and forward.** The server holds nothing. A message published while its
recipient is offline is gone. Both parties have to be connected at the same time.

**TLS certificates.** None are needed. Every connection is encrypted and
authenticated by the Noise handshake using the peer identities, so there is
nothing to obtain, renew, or configure. A reverse proxy in front of this server
is not useful and will break the handshake.

**Logging configuration.** The server prints to standard output and that is all.
`docker logs feed-server` is the interface.

**A metrics endpoint.** Nothing is exported. What a client can see about a server
it uses is in the app, under Settings, including round trip time and the reason
for a failed connection.
