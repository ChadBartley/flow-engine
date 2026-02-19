#!/usr/bin/env bash
# Setup script for ClearGate Node.js observability examples.
# Run from this directory: ./setup.sh

set -euo pipefail

REPO_ROOT="$(cd "../../.." && pwd)"
SDK_DIR="$REPO_ROOT/sdks/node"

echo "==> Building cleargate-node native addon (Rust)..."
cargo build --release -p cleargate-node --manifest-path "$REPO_ROOT/Cargo.toml"

# Determine the shared library extension for the current platform
case "$(uname -s)" in
  Linux*)  LIB_EXT="so" ;;
  Darwin*) LIB_EXT="dylib" ;;
  *)       echo "Unsupported platform: $(uname -s)"; exit 1 ;;
esac

NATIVE_LIB="$REPO_ROOT/target/release/libcleargate_node.$LIB_EXT"
if [ ! -f "$NATIVE_LIB" ]; then
  echo "ERROR: Expected native library not found at $NATIVE_LIB"
  exit 1
fi

echo "==> Copying native binary to SDK..."
cp "$NATIVE_LIB" "$SDK_DIR/cleargate.node"

echo "==> Building TypeScript SDK..."
cd "$SDK_DIR"
npm install
npm run build

echo "==> Installing example dependencies (clean)..."
cd "$REPO_ROOT/examples/observability/node"
rm -rf node_modules package-lock.json
npm install --install-links

echo ""
echo "Done! Run examples with:"
echo "  npm run basic"
echo "  npm run langchain"
echo "  npm run langgraph"
