#!/bin/bash
set -e

echo "Formatting Rust code with cargo fmt..."
cargo fmt --all
echo "Clippy fix..."
cargo clippy --all --fix --allow-dirty

echo "Formatting Python code..."
if command -v ruff &> /dev/null; then
    ruff format crates/cleargate-python/python/ crates/cleargate-python/tests/ examples/observability/python/ examples/orchestration/python/
    ruff check --fix crates/cleargate-python/python/ crates/cleargate-python/tests/ examples/observability/python/ examples/orchestration/python/
else
    echo "  ruff not installed, skipping Python formatting"
fi

echo "Formatting TypeScript/JavaScript code..."
if command -v npx &> /dev/null; then
    TS_PATHS=("sdks/node/src/**/*.ts" "examples/orchestration/node/src/**/*.ts")
    if [ -f "sdks/node/node_modules/.bin/prettier" ]; then
        npx --prefix sdks/node prettier --write "${TS_PATHS[@]}"
    elif command -v prettier &> /dev/null; then
        prettier --write "${TS_PATHS[@]}"
    else
        echo "  prettier not installed, skipping TypeScript formatting"
    fi
else
    echo "  npx not available, skipping TypeScript formatting"
fi

echo "Formatting C# code..."
if command -v dotnet &> /dev/null; then
    for csproj in sdks/dotnet/Cleargate/Cleargate.csproj examples/orchestration/dotnet/Cleargate.Orchestration.Examples.csproj; do
        if [ -f "$csproj" ]; then
            dotnet format "$csproj" --no-restore 2>/dev/null || echo "  dotnet format failed for $csproj, skipping"
        fi
    done
else
    echo "  dotnet not installed, skipping C# formatting"
fi
