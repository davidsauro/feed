# Running a relay server with Docker

A complete walkthrough from a bare Linux command line. Nothing here needs this
repository, a graphical session, or a Docker Hub account.

For what each setting means, see [CONFIGURATION.md](CONFIGURATION.md).

## 1. Check Docker is there

```bash
docker --version
```

If not:

```bash
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker "$USER"
newgrp docker
```

## 2. Decide how people will reach this machine

This is the one thing you cannot skip, and getting it wrong is the single most
common way a relay ends up half working. Write down:

- **The hostname or public IP** people will dial.
- **The port**, 4001 unless you choose otherwise.

To find the public IP of the machine you are on:

```bash
curl -s https://api.ipify.org
```

If the server sits behind a router, forward TCP 4001 to it, and make sure the
host firewall allows it:

```bash
sudo ufw allow 4001/tcp        # if you use ufw
```

## 3. Make a directory for the server's identity

```bash
mkdir -p ~/feed-server/data
cd ~/feed-server
```

The container runs as uid 1000, the first account on most Linux systems, so a
directory you made yourself is usually already writable by it. Check with `id -u`
if unsure. If you are not 1000, see [Not uid 1000](#not-uid-1000) below.

## 4. Write the configuration

**This file is optional for chat and required for file transfers.** A server
listening on `0.0.0.0` inside a container has no idea how anyone reaches it, so
it cannot tell clients where to be reached and every reservation fails.

Replace the address with yours from step 2. Use `/dns4/` for a hostname, `/ip4/`
for a bare IP:

```bash
cat > data/feed-server.toml <<'EOF'
# How people reach this machine. Required for file transfers.
external_addresses = ["/dns4/relay.example.com/tcp/4001"]

[relay]
# No limit on how much one relayed connection may carry. Right when the people
# using this server are people you know. See below to close it down.
max_circuit_bytes = 0
max_circuit_duration_secs = 86400
EOF
```

## 5. Start it

```bash
docker run -d --name feed-server \
  -p 4001:4001 \
  -v "$PWD/data:/data" \
  --restart unless-stopped \
  ghcr.io/davidsauro/feed-server:latest

docker logs -f feed-server
```

A healthy start looks like this:

```
this server is 12D3KooWMePzG8LNB4WwKqHX3b9d2yu8atwv539E5gJ2Yw7zxaoV
open: any peer may connect
relaying connections: up to 512 reservation(s), no limit per circuit, 1440 minute(s) each
reachable at /dns4/relay.example.com/tcp/4001
listening on /ip4/0.0.0.0/tcp/4001/p2p/12D3KooWMePzG8LNB4WwKqHX3b9d2yu8atwv539E5gJ2Yw7zxaoV
```

Check for two things. **`reachable at`** must show your public address, not
`0.0.0.0`. And there must be no `WARNING` about no external address being known.

## 6. The address to give people

Take the peer id from `this server is`, and put your own hostname in front of it:

```
/dns4/relay.example.com/tcp/4001/p2p/12D3KooWMePzG8LNB4WwKqHX3b9d2yu8atwv539E5gJ2Yw7zxaoV
```

That whole string goes into the app under Settings, Servers. The `/p2p/` part is
not decoration. It is how a client knows the machine answering is yours and not
whatever else holds that name today.

## 7. Check it from another machine

```bash
nc -zv relay.example.com 4001
```

From a clone of this repository, the two probes check more than a port:

```bash
# Carries conversations
cargo run -p feed-server --example probe -- <server address> /direct/1.0.0/test
cargo run -p feed-server --example probe -- <server address> /direct/1.0.0/test "hello"

# Relays connections, which is what file transfers need
cargo run -p feed-server --example circuit_probe -- <server address> 25
```

If the first passes and the second fails, `external_addresses` is wrong or
missing. That is the failure that leaves chat working perfectly.

## Closing it to a known group

By default anyone who finds this server may use it. To restrict it, collect each
person's peer id, which the app shows at the top of its sidebar and copies when
clicked:

```bash
cat >> data/feed-server.toml <<'EOF'

allowed_peers = [
  "12D3KooWEXAMPLEonlyREPLACEwithTHEIRrealPEERid00000000",
]
EOF

docker restart feed-server
```

An empty list means open, so this only takes effect once there is a name in it.
Configuration is read at startup, so a restart is always needed.

## Backing up

The whole of it is one file of 68 bytes:

```bash
cp data/identity.bin ~/somewhere-safe/
```

Nothing else is kept. No messages, no files, no record of who talked to whom.
The directory exists so the server's identity survives a restart, and it does not
grow.

Losing that file makes every client configuration wrong, because the peer id they
hold no longer matches. Anyone who copies it can impersonate this server, so
treat the backup like the private key it is.

### Moving to another machine

```bash
docker stop feed-server                     # on the old machine
scp data/identity.bin newhost:~/feed-server/data/
# point DNS at the new machine, then start it there
```

Every client keeps working. **Never run two servers with the same identity at
once**, or they will fight over connections and confuse every peer that reaches
either.

## Updating

```bash
docker pull ghcr.io/davidsauro/feed-server:latest
docker stop feed-server && docker rm feed-server
# then the docker run from step 5 again
```

The identity is on the volume, so the server comes back as itself.

## Troubleshooting

**`could not write identity.bin: Permission denied`, restarting forever.** The
container cannot write to `data/`. See below.

**Chat works, file transfers do not.** Almost always `external_addresses`.
Confirm the logs say `reachable at <your address>` and that the address is one
that actually resolves to this machine. `docker logs feed-server | grep reachable`

**A transfer stops partway through, every time, at about the same point.** A
circuit byte limit. `max_circuit_bytes` covers a whole relayed connection rather
than one file, so it is reached sooner than people expect. Set it to `0`.

**Clients cannot connect at all.** Port not reachable. Check `docker ps` shows
`0.0.0.0:4001->4001/tcp`, then the host firewall, then the router.

**A configuration change did nothing.** It is read at startup only.
`docker restart feed-server`.

**The server exits complaining about the configuration.** A file that exists but
cannot be parsed stops the server rather than being guessed at, including for a
misspelled key. The message names the problem.

### Not uid 1000

If `id -u` is not 1000, the container cannot write to a directory you own. Give
it away:

```bash
sudo chown -R 1000:1000 data
```

Or build an image that runs as you, from a clone:

```bash
cd crates/server
UID=$(id -u) GID=$(id -g) docker compose up -d --build
```
