# Testing file transfers across a relay

For two machines and a server. Written to be worked through in order, because
each part rests on the one before it and finding out that step 9 fails is far
less useful when step 3 was already broken.

Set the server up first with [crates/server/DOCKER.md](crates/server/DOCKER.md).

Throughout: **A** and **B** are the two machines, **S** is the server.

## Before anything else

| Check | How | Expect |
|---|---|---|
| S is up | `docker logs indicium-server` | `reachable at <your address>`, no `WARNING` |
| S carries conversations | `probe` example, twice | The message arrives |
| S relays connections | `circuit_probe` example | `OK: 25 MiB crossed the relay` |
| A reservation survives the app's ordering | `reserve_probe` example | `RESERVED: …/p2p-circuit/…` |
| Messages cross quickly | `latency_probe` example | Single or low double digit ms |

If `circuit_probe` fails where `probe` passed, stop. `external_addresses` is
wrong, and nothing below will work.

`latency_probe` times a message across the server. Anything approaching a second
means the server is older than its clients and cannot forward directly, so every
message is waiting on a gossip round instead.

`reserve_probe` exists because the app dials a server first and asks to be
reachable through it second, which the other probes do not. Run it with
`during` instead of `after` to see the failure that ordering used to cause: a
reservation that never happens, reported as a listener closing successfully.

## 1. Local network, no server involved

Put A and B on the same network. Do not configure any server yet.

1. They find each other, both showing under Discovered.
2. Add each other as contacts. **Both sides must add**, since a conversation only
   reaches somebody listening for it.
3. Send a message each way.
4. Send a small file, A to B. It arrives, opens, and appears in Files under B's
   name on A and A's name on B.
5. **Send a file larger than 25 MB.** It should go through. There is no limit on
   this path, and if you are told there is one, the direct connection is not
   being recognised.

This is the path that worked before any of the relay work, so a failure here is a
regression rather than something new.

## 2. Server configured, still on the same network

Add S to both A and B under Settings, Servers.

1. The server list shows **Online** with a round trip figure.
2. The sidebar shows `1/1` with a green dot.
3. **Test connections** reports Online within a few seconds.
4. Messages still work.
5. **A file over 25 MB still goes through.** Being connected to a server must not
   impose the relayed limit while a direct connection exists. If this now fails,
   the size split is reading the wrong thing.

## 3. Separate networks, the real case

Move B somewhere else. A phone hotspot is the easiest way, and using a different
carrier from A's network is better than the same one.

**Adding each other now takes the peer id**, since discovery does not leave a
local network. On each machine, click the id at the top of the sidebar to copy
it, send it across, and use **Add someone by their ID** under Discovered. Both
sides must do it.

Copy it. Do not read it off the screen and type it. Every peer id starts
`12D3KooW`, so a wrong one looks entirely plausible, and the result is two people
subscribed to different conversations with no obvious sign of it. If you want to
be sure, `docker logs indicium-server` shows a `carrying` line per conversation, and
two people who have added each other correctly share one: the second to connect
produces one fewer line than they have contacts.

1. Both still show their server Online.
2. Messages work in both directions. This is the existing relay path and does not
   involve any of the new work.
3. Presence: A and B should show each other online.

### 3a. A small file, B to A

The first genuinely new thing. Expect it to take a few seconds longer to start
than on a local network, because a connection has to be built through S first.

- It completes.
- The hash check passes, meaning no error about the file not matching.
- It opens on the receiving side.

### 3b. A small file, A to B

The reverse direction. Worth doing separately: the receiver dials the sender, so
the two directions exercise different halves of the address exchange.

### 3c. A file over 25 MB

Should be **refused before sending**, in the picker, saying it is too large to
send through a relay. Not accepted and then failed partway.

### 3d. A file just under 25 MB

Should go through. Watch the progress bar move steadily rather than in one jump
at the end.

## 4. Interruptions

The parts most likely to be wrong, since they are the least exercised.

### 4a. Pull the network mid-transfer

Start a large transfer from 3d, then disconnect B's network for about ten seconds
and reconnect it.

- The transfer should **resume rather than restart**. Watch the progress figure:
  it should carry on from roughly where it stopped.
- Up to five attempts are made, five seconds apart. Beyond that it gives up and
  says so.

### 4b. Close the laptop lid mid-transfer

The same thing by a different route, and the one likely to behave differently,
because the whole process is suspended rather than just the network.

### 4c. Restart the app mid-transfer

Stop the receiving app partway and start it again.

- On restart it should read as **interrupted, and can be resumed**, not as still
  receiving. A transfer that was in flight when the app stopped is not in flight
  now, and saying otherwise is the interface reporting something that was true
  once.
- A **Resume** button should be on that row. Pressing it asks for the rest and
  the progress bar should carry on from where it stopped rather than from zero.
- Pressing it while the sender is **offline** should say `reaching them`, then
  `no answer, trying again (2 of 5)` and so on. Five attempts, each able to
  spend a full dial timeout, so it keeps trying for the better part of three
  minutes. Bringing the sender back within that window picks the transfer up
  without pressing anything again.
- The sender has a Resume button too. Pressing it offers the same file again
  rather than sending a second copy, and the receiver picks up from where it
  stopped. Either side can start it off.
- Pressing it while the **receiver** is offline should count out
  `waiting for them (1 of 4)` and end at `they are not answering`, with the
  Resume button back. An offer is an ordinary message and nothing stores those,
  so one sent to a node that is not running is gone: it does not go through when
  they return, and a row that says it is waiting is saying something untrue.
- Failures should read as a short phrase, not a paragraph. The full text is on
  hover. A relay failure in particular arrives as three hundred characters of
  nested causes and two addresses, and a row is not the place for it.

### 4d. Restart the server mid-transfer

`docker restart indicium-server`. The transfer will fail. Once the server is back,
both clients should reconnect within 30 seconds and a new transfer should work.

## 5. Circuit limits

Only if you want to see the failure mode described in the docs. On S:

```bash
# a deliberately small budget
sed -i 's/max_circuit_bytes = 0/max_circuit_bytes = 2097152/' data/indicium-server.toml
docker restart indicium-server
```

Send a 10 MB file. It should stop around 2 MB, then **retry and resume**,
possibly several times, and eventually complete. This is the behaviour that makes
a byte budget a nuisance rather than a wall.

Put it back to `0` afterwards.

## 6. Several files, and both directions at once

1. Stage several files in the Files view and send them as one batch.
2. Send from A to B and B to A at the same time.
3. Check the Files view groups them under the right contact, with counts and a
   total size that look right.

## 6b. Files to a group

Needs a group with at least two other members, one of which you can take offline.

1. Open the group and use the attach button, or Add files on its section in the
   Files view. Send two or three files at once.
2. In the Files view the group gets a section of its own, above the one to one
   history, with **one line per member** rather than one per file. Three members
   and three files is three lines, not nine.
3. A member's line should say how far along they are and, while something is
   moving, which file. Opening it with the arrow shows their files.
4. **Take one member offline and send again.** Everyone else should complete.
   The absent member should count out its attempts and end at "they are not
   answering", and their line should say something did not go, in red, without
   needing to be opened.
5. **Resume on that member's line** should retry only their failures. Nobody
   else is disturbed.
6. Bring them back and press it again. It should go through, resuming any
   partial file rather than starting over.
7. Check the size limit: a file over 25 MB should be refused if **any** member
   has to be reached through a relay, even when the others are on this network.

## 7. Things that should fail cleanly

Failure is fine. Failing in a way nobody can act on is not.

| Do this | Expect |
|---|---|
| Add a server address with no `/p2p/` | Refused when you add it, saying what is missing |
| Add a server that is not running | Listed as Offline with a reason, not a hang |
| Send to a contact who is offline | Says so, does not sit at 0% forever |
| Remove a contact who sent you files | Conversation goes, files stay, shown under their peer id |
| Stop the server while chatting | Both show the server Offline within about 30 seconds |

## What to write down

For anything that fails, this is what makes it fixable:

- Which step, and which machine was sending.
- Whether A and B were on the same network.
- The exact message shown.
- The last few lines of `docker logs indicium-server`.
- Whether it fails every time or sometimes. **Sometimes is the more important
  answer**, and the one people forget to check.

## Known gaps before you start

Not bugs, and worth knowing so they are not reported as such:

- **A transfer interrupted by closing the app does not resume by itself.** It is
  marked resumable on the next start and there is a Resume button, but nothing
  picks it up unprompted.
- **No hole punching.** Every relayed byte crosses the server, so a transfer
  between two machines in the same building still goes out to S and back if they
  cannot see each other directly. DCUtR would fix that and is not written.
- **A group's files appear in the Files view, not in the group conversation.**
  One file sent to fifteen people is fifteen transfers, and showing that in a
  conversation would show the same file fifteen times.
- **Nothing checks free disk space**, so an unlimited transfer on a local network
  can fill a disk.
