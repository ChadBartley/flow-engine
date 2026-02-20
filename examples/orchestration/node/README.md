# Cleargate Orchestration — Node.js Examples

Run flow graphs through the Cleargate engine from Node.js/TypeScript.

## Prerequisites

1. Build the Node.js native module:
   ```bash
   cd crates/cleargate-node && npm run build
   ```

2. Install SDK dependencies:
   ```bash
   cd sdks/node && npm install && npm run build
   ```

## Examples

| # | Script | Description |
|---|--------|-------------|
| 01 | `src/01_basic_flow.ts` | Load and execute a simple echo flow |
| 02 | `src/02_streaming_events.ts` | Stream events using async generator |
| 03 | `src/03_execute_graph.ts` | Execute an ad-hoc graph |

## Running

```bash
npx ts-node src/01_basic_flow.ts
```
