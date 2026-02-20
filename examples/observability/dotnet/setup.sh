#!/usr/bin/env bash
# Setup script for ClearGate .NET observability examples.
# Run from this directory: ./setup.sh

set -euo pipefail

REPO_ROOT="$(cd "../../.." && pwd)"
EXAMPLE_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "==> Building cleargate-dotnet native library (Rust)..."
cargo build --release -p cleargate-dotnet --manifest-path "$REPO_ROOT/Cargo.toml"

# Determine the shared library name for the current platform
case "$(uname -s)" in
  Linux*)  LIB_FILE="libcleargate_dotnet.so" ;;
  Darwin*) LIB_FILE="libcleargate_dotnet.dylib" ;;
  *)       echo "Unsupported platform: $(uname -s)"; exit 1 ;;
esac

NATIVE_LIB="$REPO_ROOT/target/release/$LIB_FILE"
if [ ! -f "$NATIVE_LIB" ]; then
  echo "ERROR: Expected native library not found at $NATIVE_LIB"
  exit 1
fi

echo "==> Restoring .NET dependencies..."
cd "$EXAMPLE_DIR"
dotnet restore

# Copy the native library next to the build output so .NET can find it
OUTPUT_DIR="$EXAMPLE_DIR/bin/Debug/net8.0"
mkdir -p "$OUTPUT_DIR"
cp "$NATIVE_LIB" "$OUTPUT_DIR/"

echo ""
echo "Done! Run examples with:"
echo "  dotnet run -- basic"
echo "  dotnet run -- semantic-kernel"
echo "  dotnet run -- tool-call"
