#!/usr/bin/env python3
"""Execute Ad-hoc Graph

Demonstrates executing a graph definition directly without storing it
in the flow store first. Useful for testing and one-off executions.
"""

import json
import os
import tempfile

from cleargate import EngineBuilder


def main():
    with tempfile.TemporaryDirectory() as tmpdir:
        flow_dir = os.path.join(tmpdir, "flows")
        run_dir = os.path.join(tmpdir, "runs")
        os.makedirs(flow_dir, exist_ok=True)

        builder = EngineBuilder()
        builder.flow_store_path(flow_dir)
        builder.run_store_path(run_dir)
        builder.crash_recovery(False)
        engine = builder.build()

        # Define a graph inline — two jq nodes chained together
        graph = {
            "schema_version": 1,
            "id": "adhoc-pipeline",
            "name": "Ad-hoc Pipeline",
            "version": "1.0",
            "nodes": [
                {
                    "instance_id": "step1",
                    "node_type": "jq",
                    "config": {
                        "expression": '{greeting: ("Hello, " + (.name // "World") + "!")}'
                    },
                },
                {
                    "instance_id": "step2",
                    "node_type": "jq",
                    "config": {
                        "expression": "{result: .greeting, length: (.greeting | length)}"
                    },
                },
            ],
            "edges": [
                {
                    "id": "e1",
                    "from_node": "step1",
                    "from_port": "output",
                    "to_node": "step2",
                    "to_port": "input",
                }
            ],
            "tool_definitions": {},
            "metadata": {},
        }

        print("Executing ad-hoc graph...")
        handle = engine.execute_graph(graph, {"name": "Cleargate"})
        print(f"Run ID: {handle.run_id}")

        for event in handle:
            for event_type, payload in event.items():
                print(f"  [{event_type}]")
                if event_type == "NodeCompleted" and isinstance(payload, dict):
                    node_id = payload.get("node_instance_id", "")
                    outputs = payload.get("outputs", {})
                    print(f"    node={node_id} outputs={json.dumps(outputs)}")

        engine.shutdown()
        print("\nDone!")


if __name__ == "__main__":
    main()
