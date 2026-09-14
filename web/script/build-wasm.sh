#!/bin/bash
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Building Goop Web...${NC}"

# Get the script directory
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
PROJECT_ROOT="$SCRIPT_DIR/.."

# Parse arguments
RELEASE_FLAG=""
if [[ "$1" == "--release" ]]; then
    RELEASE_FLAG="--release"
    echo -e "${YELLOW}Building in release mode${NC}"
fi

# macOS: ensure LLVM from Homebrew is available
if [[ "$(uname)" == "Darwin" ]]; then
    echo -e "${GREEN}Detected macOS, setting up LLVM...${NC}"
    if brew list llvm &>/dev/null; then
        echo -e "${GREEN}LLVM already installed, skipping install${NC}"
    else
        echo -e "${YELLOW}LLVM not found, installing via Homebrew...${NC}"
        brew install llvm
    fi
    LLVM_PATH=$(brew --prefix llvm)
    export AR="${LLVM_PATH}/bin/llvm-ar"
    export CC="${LLVM_PATH}/bin/clang"
    echo -e "${GREEN}LLVM path: ${LLVM_PATH}${NC}"
fi

# Step 1: Build WASM
echo -e "${GREEN}Step 1: Building WASM...${NC}"
cd "$PROJECT_ROOT"
export CARGO_TARGET_DIR="$PROJECT_ROOT/target"
RUSTFLAGS='-C target-feature=+bulk-memory -C link-arg=--max-memory=4294967296' \
cargo build --target wasm32-unknown-unknown $RELEASE_FLAG

# Determine the build directory
if [[ "$RELEASE_FLAG" == "--release" ]]; then
    BUILD_MODE="release"
else
    BUILD_MODE="debug"
fi

# WASM file is in workspace target directory
WASM_PATH="$PROJECT_ROOT/target/wasm32-unknown-unknown/$BUILD_MODE/goop_web.wasm"

# Check if WASM file exists
if [[ ! -f "$WASM_PATH" ]]; then
    echo -e "${RED}Error: WASM file not found at: $WASM_PATH${NC}"
    exit 1
fi

# Step 2: Generate JavaScript bindings
echo -e "${GREEN}Step 2: Generating JavaScript bindings...${NC}"
wasm-bindgen "$WASM_PATH" \
    --out-dir "$PROJECT_ROOT/www/wasm" \
    --target web \
    --no-typescript

echo -e "${GREEN}✓ Build completed successfully!${NC}"
echo -e "${YELLOW}Next steps:${NC}"
echo -e "  cd www"
echo -e "  deno install"
echo -e "  deno run dev"
