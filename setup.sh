#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

echo "=== Updating system and installing Tauri dependencies ==="
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  build-essential \
  curl \
  wget \
  file \
  libssl-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev

echo "=== Installing NVM (Node Version Manager) ==="
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash

# Load NVM into the current shell execution context
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && \. "$NVM_DIR/nvm.sh"

echo "=== Installing the latest Node.js LTS via NVM ==="
nvm install --lts
nvm use --lts

echo "=== Installing the Rust toolchain ==="
# The -y flag skips the interactive installation prompt
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

# Load Cargo into the current shell execution context
source "$HOME/.cargo/env"

echo "=== Setup Complete! ==="
echo "Node version: $(node -v)"
echo "NPM version: $(npm -v)"
echo "Rust version: $(rustc --version)"