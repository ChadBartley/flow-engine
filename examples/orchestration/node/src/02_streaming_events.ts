/**
 * Streaming Events
 *
 * Demonstrates real-time event streaming by polling nextEvent()
 * until the execution completes (returns null).
 */

import * as fs from "fs";
import * as path from "path";
import * as os from "os";

const native = require("../../../crates/cleargate-node/cleargate.node");

function main() {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "cleargate-"));
  const flowDir = path.join(tmpDir, "flows");
  const runDir = path.join(tmpDir, "runs");

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

    console.log("Executing echo-flow with streaming events...\n");
    const handle = engine.execute("echo-flow", { message: "Stream test" });

    const start = Date.now();
    let event;
    while ((event = handle.nextEvent()) !== null) {
      const elapsed = ((Date.now() - start) / 1000).toFixed(3);
      const type = Object.keys(event)[0];
      const payload = event[type];
      console.log(`  [${elapsed}s] ${type}`);
      if (payload?.node_instance_id) {
        console.log(`           node: ${payload.node_instance_id}`);
      }
    }

    console.log(
      `\nAll events received in ${((Date.now() - start) / 1000).toFixed(3)}s`,
    );
    engine.shutdown();
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

main();
