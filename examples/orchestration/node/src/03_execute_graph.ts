/**
 * Execute Ad-hoc Graph
 *
 * Demonstrates executing a graph definition directly without storing
 * it in the flow store first.
 */

import * as fs from "fs";
import * as path from "path";
import * as os from "os";

const native = require("../../../crates/cleargate-node/cleargate.node");

function main() {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "cleargate-"));
  const flowDir = path.join(tmpDir, "flows");
  const runDir = path.join(tmpDir, "runs");
  fs.mkdirSync(flowDir, { recursive: true });

  try {
    const builder = new native.NapiEngineBuilder();
    builder.flowStorePath(flowDir);
    builder.runStorePath(runDir);
    builder.crashRecovery(false);
    const engine = builder.build();

    const graph = {
      schema_version: 1,
      id: "adhoc-pipeline",
      name: "Ad-hoc Pipeline",
      version: "1.0",
      nodes: [
        {
          instance_id: "step1",
          node_type: "jq",
          config: {
            expression: '{greeting: ("Hello, " + (.name // "World") + "!")}',
          },
        },
        {
          instance_id: "step2",
          node_type: "jq",
          config: {
            expression: "{result: .greeting, length: (.greeting | length)}",
          },
        },
      ],
      edges: [
        {
          id: "e1",
          from_node: "step1",
          from_port: "output",
          to_node: "step2",
          to_port: "input",
        },
      ],
      tool_definitions: {},
      metadata: {},
    };

    console.log("Executing ad-hoc graph...");
    const handle = engine.executeGraph(graph, { name: "Cleargate" });
    console.log(`Run ID: ${handle.runId}`);

    let event;
    while ((event = handle.nextEvent()) !== null) {
      const type = Object.keys(event)[0];
      const payload = event[type];
      console.log(`  [${type}]`);
      if (type === "NodeCompleted" && payload?.outputs) {
        console.log(
          `    node=${payload.node_instance_id} outputs=${JSON.stringify(payload.outputs)}`,
        );
      }
    }

    engine.shutdown();
    console.log("\nDone!");
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

main();
