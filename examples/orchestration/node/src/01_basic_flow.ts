/**
 * Basic Flow Execution
 *
 * Demonstrates the minimal engine setup:
 * 1. Create an EngineBuilder
 * 2. Point it at flow/run store directories
 * 3. Build the engine
 * 4. Execute a flow and collect all events
 */

import * as fs from "fs";
import * as path from "path";
import * as os from "os";

// The native binding is loaded via require() by the SDK
const native = require("../../../crates/cleargate-node/cleargate.node");

function main() {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "cleargate-"));
  const flowDir = path.join(tmpDir, "flows");
  const runDir = path.join(tmpDir, "runs");

  // Copy echo flow
  const flowDest = path.join(flowDir, "echo-flow");
  fs.mkdirSync(flowDest, { recursive: true });
  fs.copyFileSync(
    path.join(__dirname, "../../flows/echo-flow.json"),
    path.join(flowDest, "flow.json"),
  );

  try {
    const builder = new native.NapiEngineBuilder();
    builder.flowStorePath(flowDir);
    builder.runStorePath(runDir);
    builder.crashRecovery(false);
    const engine = builder.build();

    console.log("Engine built. Node catalog:");
    const catalog = engine.nodeCatalog();
    for (const node of catalog) {
      console.log(`  - ${node.node_type} (${node.category})`);
    }

    console.log("\nExecuting echo-flow...");
    const handle = engine.execute("echo-flow", {
      message: "Hello from Node.js!",
    });
    console.log(`Run ID: ${handle.runId}`);

    const events = handle.waitForCompletion();
    console.log(`\nReceived ${events.length} events:`);
    for (const event of events) {
      const type = Object.keys(event)[0];
      console.log(`  [${type}]`);
    }

    engine.shutdown();
    console.log("\nDone!");
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

main();
