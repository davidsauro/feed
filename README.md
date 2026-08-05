# Local Mesh P2P Node

A decentralized, peer-to-peer communication application built with a web frontend and a native Rust networking core. This application acts as both a client and a server (a "host node") to allow users to send and receive data resiliently over local networks.

## Tech Stack
* **Frontend:** Vue 3 (Composition API) + Vite
* **Backend / System IPC:** Tauri 2.0
* **Networking / Crypto:** Rust + `rust-libp2p` + Tokio
* **Database:** SQLite (`rusqlite`)

## Current Features
* **Cryptographic Identity:** Uses `rust-libp2p` to generate an Ed25519 keypair, uniquely identifying the node on the network.
* **Local Peer Discovery:** Automatically discovers and connects to other active nodes on the local network using the mDNS protocol.
* **Persistent Storage & Address Book:** Uses local SQLite databases to save recognized contacts, custom nicknames, and chat history natively to the host OS. Contacts can be renamed, or removed along with their chat history in a single transaction (with a confirmation warning, since there is no undo).
* **Spam-Resistant Direct Messaging:** Secure 1-to-1 chat using the `libp2p-request-response` protocol. The backend performs a database lookup to silently drop inbound messages from non-contacts.
* **Group Messaging:** Multi-party chat over `libp2p-gossipsub`, one topic per group. Messages relay through intermediate peers, so members don't need to be directly connected. Gossipsub signs every message, so the same contact-based spam filter applies to the peer who actually wrote it rather than whoever relayed it. Groups are created by inviting contacts over the direct channel — gossipsub only delivers to peers already subscribed, so an invite is what bootstraps membership; the member list then rides along with each message to keep everyone in step. Group messages report sent or failed, but not read: gossipsub can tell us a message reached the mesh, not who saw it.
* **Rich Chat UI:** Features real-time online/offline status indicators and Telegram-style read receipts (Sending / Delivered / Read) driven by hidden JSON network payloads and SVGs.
* **Asynchronous IPC Bridge:** Utilizes Tokio `mpsc` channels and Tauri State to seamlessly pass network events between the background Rust network loop and the reactive Vue frontend without race conditions.

---

## Development Setup

This project is currently optimized for development on Windows 11 using WSL2 (Ubuntu 24.04).

### 1. Environment Preparation
A setup script is included to automatically install all required Linux C-dependencies (for Tauri's webview), NVM, Node.js LTS, and the Rust toolchain.

From the root of the repository, make the script executable and run it:

chmod +x setup.sh
./setup.sh

Once the script finishes, reload your terminal profile to ensure the Node and Rust paths are registered:

source ~/.bashrc

### 2. Install Project Dependencies
Install the required frontend packages using NPM:

npm install

### 3. Run the Application
Launch the Tauri development server. This will compile the Rust backend, start the Vite frontend, and open the application window via WSLg:

npm run tauri dev

> **Note for WSL2 Users:** If you see `libEGL` or `MESA` warnings in your terminal and want to suppress them, or if the window fails to render properly, force software rendering by prepending this environment variable:
> `LIBGL_ALWAYS_SOFTWARE=1 npm run tauri dev`
>
> **Theme detection under WSL2:** the webview is `webkit2gtk`, so CSS `prefers-color-scheme` reports the GTK theme inside your Linux distro rather than the Windows setting — it will say "light" even when Windows is in dark mode. Use the Light / Dark switch in the bottom of the sidebar; the choice is saved and overrides detection. Setting a dark GTK theme (`gsettings set org.gnome.desktop.interface color-scheme prefer-dark`) makes the "System" option work too.

---

## Running Multiple Instances Locally

To test the P2P mesh network on a single machine, you can spin up multiple instances of the application simultaneously. 

Use the `NODE_ID` and `APP_PORT` environment variables to prevent port collisions and ensure each instance generates a unique cryptographic identity and isolated SQLite database.

**Terminal 1 (Node 1):**
`APP_PORT=1420 NODE_ID=1 npm run tauri dev -- --port 1420`

**Terminal 2 (Node 2):**
`APP_PORT=1421 NODE_ID=2 npm run tauri dev -- --port 1421`

**Terminal 3 (Node 3):**
`APP_PORT=1422 NODE_ID=3 npm run tauri dev -- --port 1422`

The UI includes an Instance indicator in the bottom right corner to help keep track of which window belongs to which terminal.

---

## Project Structure

* `/src`: Contains the Vue frontend code.
  * `/src/App.vue`: Application shell. Owns all state and every call into the Rust backend.
  * `/src/components/`: Presentational components — `IdentityBar`, `ContactList`, `GroupList`, `PeerList`, `ChatPane`, `MessageBubble`, `ThemeToggle`, `ConfirmDialog`, `NewGroupDialog`.
  * `/src/styles.css`: Theme tokens (light and dark) and base styles. Every color in the app comes from here.
  * `/src/theme.ts`: Resolves the light/dark choice and applies it as `<html data-theme>`. Persists to `localStorage`.
  * `/src/types.ts`: Types shared between `App.vue` and its components.
* `/src-tauri`: Contains the Rust backend code.
  * `/src-tauri/src/main.rs`: The entry point for the Rust backend, SQLite initialization, and Tauri IPC commands.
  * `/src-tauri/Cargo.toml`: Rust dependencies (including libp2p, tokio, and rusqlite).
* `setup.sh`: Automated environment bootstrapping script.
