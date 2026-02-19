#!/bin/bash
set -e

echo "Formatting Rust code with cargo fmt..."
cargo fmt --all
echo "Clippy fix..."
cargo clippy --all --fix --allow-dirty

echo "Formatting Python code..."
if command -v ruff &> /dev/null; then
    ruff format crates/cleargate-python/python/ crates/cleargate-python/tests/
    ruff check --fix crates/cleargate-python/python/ crates/cleargate-python/tests/
else
    echo "  ruff not installed, skipping Python formatting"
fi

echo "Formatting TypeScript/JavaScript code..."
if command -v npx &> /dev/null && [ -d "sdks/node" ]; then
    if [ -f "sdks/node/node_modules/.bin/prettier" ]; then
        npx --prefix sdks/node prettier --write "sdks/node/src/**/*.ts"
    elif command -v prettier &> /dev/null; then
        prettier --write "sdks/node/src/**/*.ts"
    else
        echo "  prettier not installed, skipping TypeScript formatting"
    fi
else
    echo "  npx not available or sdks/node not found, skipping TypeScript formatting"
fi

echo "Formatting C# code..."
if command -v dotnet &> /dev/null && [ -d "sdks/dotnet/Cleargate" ]; then
    dotnet format sdks/dotnet/Cleargate/Cleargate.csproj --no-restore 2>/dev/null || echo "  dotnet format failed, skipping"
else
    echo "  dotnet not installed, skipping C# formatting"
fi
