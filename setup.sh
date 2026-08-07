#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

echo "=== Updating system and installing build dependencies ==="
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
  librsvg2-dev \
  perl

# A note on two of those.
#
# perl is not for us, it is for OpenSSL. The app bundles SQLCipher and compiles
# OpenSSL from source so that encryption at rest needs no system library, and
# OpenSSL configures itself with a Perl script. Ubuntu ships perl by convention
# rather than by requirement, so a minimal install can be without it, and the
# failure then happens deep in a build script rather than anywhere obvious.
#
# libssl-dev is no longer strictly required, for the same reason: OpenSSL is
# vendored rather than linked from the system. It is left in because it costs
# nothing and other crates may want it later.

echo "=== Installing NVM (Node Version Manager) ==="
curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.6/install.sh | bash

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
echo
echo "That covers the desktop app. Building or running the relay server image"
echo "additionally needs Docker, which this script deliberately does not install:"
echo "on WSL2 it is usually better to enable Docker Desktop's integration for"
echo "this distro than to run a second daemon inside it."
echo
echo "The server itself builds with cargo alone and needs nothing extra:"
echo "  cargo run -p feed-server"