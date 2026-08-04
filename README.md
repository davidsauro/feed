# Local Mesh P2P Node

A decentralized, peer-to-peer communication application built with a web frontend and a native Rust networking core. This application acts as both a client and a server (a "host node") to allow users to send and receive data resiliently over local networks.

## Tech Stack
* Frontend: Vue 3 (Composition API) + Vite
* Backend / System IPC: Tauri 2.0
* Networking / Crypto: Rust + rust-libp2p

## Current Features
* Cryptographic Identity: Uses rust-libp2p to generate an Ed25519 keypair.
* Persistent Storage: Saves the private Identity Key securely to the host operating system's native application data folder, ensuring the PeerId remains persistent across app restarts.
* IPC Bridge: Seamlessly passes the network identity from the Rust backend to the Vue frontend.

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

Note for WSL2 Users: If you see libEGL or MESA warnings in your terminal and want to suppress them, or if the window fails to render properly, force software rendering by prepending this environment variable:

    LIBGL_ALWAYS_SOFTWARE=1 npm run tauri dev

---

## Project Structure

* /src: Contains the Vue frontend code (App.vue, styles, components).
* /src-tauri: Contains the Rust backend code.
  * /src-tauri/src/main.rs: The entry point for the Rust backend and Tauri IPC commands.
  * /src-tauri/Cargo.toml: Rust dependencies (including libp2p).
* setup.sh: Automated environment bootstrapping script.