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

## Running it

```bash
cd crates/server
mkdir -p data
docker compose up -d
docker compose logs -f
```

The container runs as an unprivileged user, uid 1000 by default, because that is
the first account created on most Linux hosts — so a directory you made yourself
is one it can already write to. If your account is not 1000, say so once:

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

## Configuration

Optional. With no configuration file the server listens on port 4001 and carries
traffic for anyone, which is a useful thing to run as it stands.

To change that, copy `feed-server.example.toml` to `data/feed-server.toml` — it
documents every setting — and restart. A file that exists but cannot be parsed
stops the server rather than being guessed at, including for an unknown key.

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
> either of them. Stop the old one before starting the new one — a migration,
> not a handover.

> **Treat `identity.bin` as the private key it is.** Anyone who copies it can be
> this server: accept your users' connections and see everything it can see. It
> is written with owner-only permissions; a backup deserves the same care.

## What an operator can and cannot see

Cannot see: what anyone says, who is in a group, what a group is called, or the
names of the people using it. All of that is sealed or never sent.

Can see: which clients are connected, and which of them are interested in the
same conversations — a conversation is an opaque string, but two clients
interested in the same one are two clients with something to say to each other.
Also message sizes and timing.

That is inherent to routing anything at all, and worth being straight about with
the people whose traffic you carry. It is also the argument for running one of
these for your own circle rather than using somebody else's.

## Limits

An open server is bandwidth anyone who finds it can spend. The defaults cap what
one client can cost: 64 conversations each, 4096 across the server, 512 incoming
connections, and a limit on half-finished handshakes, which are the cheapest
thing to flood a server with. All of them are configurable.

Not yet implemented: a cap on how *often* a client may publish. Doing that
properly means taking over message validation from gossipsub, which is a larger
change. A server exposed publicly should have it.
