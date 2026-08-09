# feed-server

Carries traffic between nodes that cannot reach each other directly.

Most people run the app from behind a home router, which will not accept
incoming connections, so two such nodes can never connect to each other however
hard they try. This one sits somewhere reachable and passes messages between
them. Every node dials *out* to it, which routers do allow.

It cannot read anything it carries. Messages are sealed for their recipients
before they leave the sending node, so what passes through here is ciphertext
this program has no keys for. It has no idea what a contact is, what a group is,
or which conversation a topic belongs to.

It does two jobs. It **carries conversations**, forwarding sealed messages for
topics its clients asked for. It also **relays connections**, so that two people
who cannot reach each other directly can still open a byte stream between them,
which is what a file transfer needs and what a topic cannot provide. The two ends
of a relayed connection complete their own encrypted handshake through it, so
this program can no more read a transfer than it can read a message.

## Running it

You do not need this repository. A published image is enough:

```bash
mkdir -p data
docker run -d --name feed-server \
  -p 4001:4001 \
  -v "$PWD/data:/data" \
  --restart unless-stopped \
  ghcr.io/davidsauro/feed-server:latest

docker logs -f feed-server
```

Images are published for `linux/amd64` and `linux/arm64`, so the cheap virtual
machines a small relay tends to live on are covered either way.

Tags: `0.1.0` and `0.1` follow releases, `latest` is whatever was published most
recently, and `sha-<commit>` pins an exact build.

### From a clone

```bash
cd crates/server
mkdir -p data
docker compose up -d
docker compose logs -f
```

The container runs as an unprivileged user, uid 1000 by default, because that is
the first account created on most Linux hosts. A directory you made yourself is
therefore one it can already write to. If your account is not 1000, say so once:

```bash
UID=$(id -u) GID=$(id -g) docker compose up -d --build
```

Getting this wrong is not subtle. The server exits immediately with
`could not write identity.bin: Permission denied`, and restarts forever.

On first start it prints the address to give people:

```
this server is 12D3KooWMePzG8LNB4WwKqHX3b9d2yu8atwv539E5gJ2Yw7zxaoV
listening on /ip4/0.0.0.0/tcp/4001/p2p/12D3KooWMePzG8LNB4WwKqHX3b9d2yu8atwv539E5gJ2Yw7zxaoV
```

Clients need the *public* form of that, with your hostname in place of the
listen address:

```
/dns4/relay.example.com/tcp/4001/p2p/12D3KooWMePzG8LNB4WwKqHX3b9d2yu8atwv539E5gJ2Yw7zxaoV
```

The peer id on the end is not decoration. It is how a client knows the machine
answering is the server it meant, and not whatever else holds that hostname
today. An address without it will be refused.

The image is about 120 MB and the running server uses around 3 MB of memory.

## Checking a server actually carries traffic

There is a probe that stands in for the app:

```bash
# a listener, and then a sender, each connected only to the server
cargo run -p feed-server --example probe -- <server address> /direct/1.0.0/test
cargo run -p feed-server --example probe -- <server address> /direct/1.0.0/test "hello"
```

The two probes are never told each other's addresses, so a message arriving at
the listener can only have gone through the server.

File transfers use a different mechanism, a relayed connection rather than a
topic, and have their own probe:

```bash
cargo run -p feed-server --example circuit_probe -- <server address> 25
```

Two peers in one process exchange 25 MiB that can only have crossed the relay.
If this fails where the first probe passed, the usual cause is
`external_addresses` being unset, since a server that does not know its own
public address cannot tell a client where to be reached. Conversations keep
working regardless, which is what makes it easy to miss.

## Configuration

Optional. With no configuration file the server listens on port 4001 and carries
traffic for anyone, which is a useful thing to run as it stands.

To change that, copy `feed-server.example.toml` to `data/feed-server.toml` and
restart. A configuration that exists but cannot be parsed stops the server rather
than being guessed at, including when the problem is nothing more than an unknown
key.

One setting is worth knowing about before you need it. Relaying file transfers
requires `external_addresses`, because a server listening on `0.0.0.0` cannot
tell a client what address to be reached at. In Docker that is always the case.
See [external_addresses](CONFIGURATION.md#external_addresses).

**[CONFIGURATION.md](CONFIGURATION.md) is the reference:** what the server does,
every setting with its default and what goes wrong if you get it wrong, worked
examples for an open relay, a private one, and a federated pair, and a list of
the things people look for that do not exist yet.

## What it stores, and how to back it up

Two files, in the mounted directory:

| File | Size | What it is |
|---|---|---|
| `identity.bin` | **68 bytes** | This server's private key |
| `feed-server.toml` | a few hundred bytes | Your configuration, if you wrote one |

That is all, and it does not grow. No messages are kept: connections, mesh state
and the record of which conversations to carry all live in memory and rebuild
themselves when clients reconnect. The mounted directory exists so the server's
*identity* survives a restart, not because there is data to accumulate.

So a backup is one command:

```bash
cp crates/server/data/identity.bin ~/somewhere-safe/
```

### Moving to different hardware

1. **Stop the old server.** `docker compose down`
2. Copy `identity.bin` to the new machine's `data/` directory.
3. Point the DNS name at the new machine.
4. Start it. `docker compose up -d`

Every client configuration keeps working, because the address they hold is the
hostname plus the identity, and both still lead to the same place.

> **Never run two servers with the same identity at once.** Two nodes claiming
> one peer id will fight over connections and confuse every peer that reaches
> either of them. Stop the old one before starting the new one. It is a
> migration, not a handover.

> **Treat `identity.bin` as the private key it is.** Anyone who copies it can be
> this server, accepting your users' connections and seeing everything it can
> see. It is written with owner-only permissions, and a backup deserves the same
> care.

## What an operator can and cannot see

Cannot see: what anyone says, who is in a group, what a group is called, or the
names of the people using it. All of that is sealed or never sent.

Can see: which clients are connected, and which of them are interested in the
same conversations. A conversation is an opaque string, but two clients
interested in the same one are two clients with something to say to each other.
Message sizes and timing are visible too.

Relaying adds a little. A circuit names both of its ends, so an operator carrying
a transfer can see which two peers are exchanging something and roughly how much
of it, though not what it is or what it is called.

That is inherent to routing anything at all, and worth being straight about with
the people whose traffic you carry. It is also the argument for running one of
these for your own circle rather than using somebody else's.

## Relaying file transfers

On by default. A client reserves a slot here, which makes it reachable at an
address naming this server and then the client, and somebody else dials that
address to open a connection through this server.

**This needs `external_addresses` set** in any deployment where the listen
address is not how people actually reach the machine, which includes every
container. Without it, reservations fail and file transfers do not work, while
conversations carry on perfectly.

Three settings govern what relaying may cost, covered in
[CONFIGURATION.md](CONFIGURATION.md#relay). The one worth reading before you set
it is `max_circuit_bytes`, which is a budget for an entire relayed connection
rather than a per-file limit. Running a relay for your own circle, no limit is
the sensible setting.

## Limits

An open server is bandwidth anyone who finds it can spend. The defaults cap what
one client can cost: 64 conversations each, 4096 across the server, 512 incoming
connections, and a limit on half-finished handshakes, which are the cheapest
thing to flood a server with. All of them are configurable, and
[CONFIGURATION.md](CONFIGURATION.md#limits) explains which to raise first.

Not yet implemented: a cap on how *often* a client may publish. Doing that
properly means taking over message validation from gossipsub, which is a larger
change. A server exposed publicly should have it.
