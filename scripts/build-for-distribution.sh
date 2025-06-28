#!/bin/bash
set -e

# Build CLI binaries for distribution via Mothership server
# This script builds for multiple platforms and organizes the binaries for serving

BOLD='\033[1m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BOLD}🔨 Building Mothership CLI for Distribution${NC}"
echo ""

# Get version from Cargo.toml
VERSION=$(grep '^version = ' mothership-cli/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
echo -e "${BLUE}📦 Building version: ${VERSION}${NC}"

# Create distribution directory
DIST_DIR="cli-binaries/${VERSION}"
mkdir -p "$DIST_DIR"

# Define target platforms
TARGETS=(
    "x86_64-unknown-linux-gnu"
    "aarch64-unknown-linux-gnu" 
    "x86_64-apple-darwin"
    "aarch64-apple-darwin"
    "x86_64-pc-windows-msvc"
)

# Install cross-compilation targets
echo -e "${YELLOW}📥 Installing cross-compilation targets...${NC}"
for target in "${TARGETS[@]}"; do
    rustup target add "$target" || echo "Target $target may already be installed"
done

# Install cross for Linux ARM64 builds
if ! command -v cross &> /dev/null; then
    echo -e "${YELLOW}📥 Installing cross for cross-compilation...${NC}"
    cargo install cross --git https://github.com/cross-rs/cross
fi

echo ""

# Build for each target
for target in "${TARGETS[@]}"; do
    echo -e "${BLUE}🔨 Building for ${target}...${NC}"
    
    # Create target directory
    mkdir -p "$DIST_DIR/$target"
    
    # Determine binary names
    if [[ "$target" == *"windows"* ]]; then
        CLI_BINARY="mothership.exe"
        DAEMON_BINARY="mothership-daemon.exe"
    else
        CLI_BINARY="mothership"
        DAEMON_BINARY="mothership-daemon"
    fi
    
    # Build CLI
    echo -e "  Building CLI..."
    if [[ "$target" == "aarch64-unknown-linux-gnu" ]]; then
        # Use cross for ARM64 Linux
        cross build --release --bin mothership --target "$target"
    else
        # Use regular cargo for other targets
        cargo build --release --bin mothership --target "$target"
    fi
    
    # Build daemon
    echo -e "  Building daemon..."
    if [[ "$target" == "aarch64-unknown-linux-gnu" ]]; then
        cross build --release --bin mothership-daemon --target "$target"
    else
        cargo build --release --bin mothership-daemon --target "$target"
    fi
    
    # Copy binaries to distribution directory
    cp "target/$target/release/$CLI_BINARY" "$DIST_DIR/$target/"
    cp "target/$target/release/$DAEMON_BINARY" "$DIST_DIR/$target/"
    
    echo -e "${GREEN}  ✅ $target complete${NC}"
done

echo ""
echo -e "${GREEN}✅ All builds completed!${NC}"
echo ""
echo -e "${YELLOW}📁 Distribution directory: ${DIST_DIR}${NC}"
echo ""
echo -e "${BLUE}Directory structure:${NC}"
find "$DIST_DIR" -type f | sort

echo ""
echo -e "${YELLOW}Next steps:${NC}"
echo -e "1. Copy the cli-binaries directory to your Mothership server"
echo -e "2. Start the Mothership server"
echo -e "3. Test installation with: curl -sSL http://your-server/cli/install | bash"
echo "" 