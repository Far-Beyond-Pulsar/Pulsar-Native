#!/bin/bash
# macOS Setup Script for Pulsar-Native
# This script installs all necessary dependencies for building Pulsar-Native on macOS

set -e  # Exit on error

echo "=================================="
echo "Pulsar-Native macOS Setup Script"
echo "=================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if running on macOS
if [[ "$OSTYPE" != "darwin"* ]]; then
    echo -e "${RED}Error: This script is for macOS only!${NC}"
    exit 1
fi

echo "Checking system requirements..."
echo ""

# Check for Xcode Command Line Tools
if ! xcode-select -p &>/dev/null; then
    echo -e "${YELLOW}Xcode Command Line Tools not found. Installing...${NC}"
    xcode-select --install
    echo "Please complete the Xcode Command Line Tools installation in the popup window."
    echo "Press any key to continue after installation completes..."
    read -n 1 -s
else
    echo -e "${GREEN}✓ Xcode Command Line Tools installed${NC}"
fi

# Check for Homebrew
if ! command -v brew &>/dev/null; then
    echo -e "${YELLOW}Homebrew not found. Installing...${NC}"
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    
    # Add Homebrew to PATH for Apple Silicon Macs
    if [[ $(uname -m) == "arm64" ]]; then
        echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
        eval "$(/opt/homebrew/bin/brew shellenv)"
    fi
else
    echo -e "${GREEN}✓ Homebrew installed${NC}"
fi

# Update Homebrew
echo "Updating Homebrew..."
brew update

# Install pkg-config (required for building dependencies)
if ! command -v pkg-config &>/dev/null; then
    echo -e "${YELLOW}Installing pkg-config...${NC}"
    brew install pkgconf
else
    echo -e "${GREEN}✓ pkg-config installed${NC}"
fi

# Install cmake (often needed for C/C++ dependencies)
if ! command -v cmake &>/dev/null; then
    echo -e "${YELLOW}Installing cmake...${NC}"
    brew install cmake
else
    echo -e "${GREEN}✓ cmake installed${NC}"
fi

# Install other common dependencies
echo "Installing additional dependencies..."
brew install zlib || echo "zlib already installed"

# Check for Rust
if ! command -v rustc &>/dev/null; then
    echo -e "${YELLOW}Rust not found. Installing via rustup...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
else
    echo -e "${GREEN}✓ Rust installed ($(rustc --version))${NC}"
fi

# Check Rust version and update if needed
echo "Checking Rust toolchain..."
rustup update

# Install required Rust targets
echo "Installing Rust targets..."
rustup target add aarch64-apple-darwin 2>/dev/null || true
rustup target add x86_64-apple-darwin 2>/dev/null || true

echo ""
echo "=================================="
echo "Environment Configuration"
echo "=================================="
echo ""

# Check for problematic environment variables
if [[ -n "$CFLAGS" ]]; then
    echo -e "${YELLOW}Warning: CFLAGS environment variable is set to: $CFLAGS${NC}"
    if [[ "$CFLAGS" == *"/std:"* ]]; then
        echo -e "${RED}Error: CFLAGS contains Windows-style flags (/std:)${NC}"
        echo "This will cause build failures. Please unset CFLAGS or change it to Unix style."
        echo ""
        echo "To fix this, add to your ~/.zshrc or ~/.bash_profile:"
        echo "  unset CFLAGS"
        echo "  # or"
        echo "  export CFLAGS=\"-std=c11\""
        echo ""
        echo "Then restart your terminal or run: source ~/.zshrc"
    fi
fi

# Create a .cargo/config.toml if it doesn't exist with proper settings
CARGO_CONFIG_DIR=".cargo"
CARGO_CONFIG_FILE=".cargo/config.toml"

if [[ ! -d "$CARGO_CONFIG_DIR" ]]; then
    mkdir -p "$CARGO_CONFIG_DIR"
fi

if [[ ! -f "$CARGO_CONFIG_FILE" ]]; then
    echo "Creating .cargo/config.toml with macOS-specific settings..."
    cat > "$CARGO_CONFIG_FILE" << 'EOF'
[build]
# Ensure we're using the correct target
target-dir = "target"

[target.aarch64-apple-darwin]
# macOS Apple Silicon specific settings
rustflags = ["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"]

[target.x86_64-apple-darwin]
# macOS Intel specific settings
rustflags = ["-C", "link-arg=-undefined", "-C", "link-arg=dynamic_lookup"]

# Environment variables for build scripts
[env]
# Ensure zlib is found
PKG_CONFIG_PATH = "/opt/homebrew/lib/pkgconfig:/usr/local/lib/pkgconfig"
EOF
    echo -e "${GREEN}✓ Created .cargo/config.toml${NC}"
else
    echo "✓ .cargo/config.toml already exists"
fi

echo ""
echo "=================================="
echo "Setup Complete!"
echo "=================================="
echo ""
echo "Next steps:"
echo "1. If you saw warnings about CFLAGS, fix them as described above"
echo "2. Restart your terminal or run: source ~/.zshrc"
echo "3. Try building the project: cargo build"
echo ""
echo -e "${GREEN}Happy coding!${NC}"
