#!/usr/bin/env python3
"""Custom Node Handler

Demonstrates registering a Python-native node handler with the engine.
The custom node receives inputs, config, and a context dict, and returns
outputs as a dict.
"""

import json
import os
import tempfile

from cleargate import EngineBuilder


class UppercaseNode:
    """A custom node that converts all string values in the input to uppercase."""

    def meta(self):
        return {
            "node_type": "uppercase",
            "label": "Uppercase",
            "category": "text",
            "inputs": [
                {
                    "name": "input",
                    "port_type": "Json",
                    "required": True,
                    "description": "Input data",
                }
            ],
            "outputs": [
                {
                    "name": "output",
                    "port_type": "Json",
                    "required": True,
                    "description": "Uppercased data",
                }
            ],
            "config_schema": {},
            "ui": {},
            "execution": {},
        }

    def run(self, inputs, config, context):
        """Convert string values to uppercase."""
        result = {}
        for key, value in inputs.items():
            if isinstance(value, str):
                result[key] = value.upper()
            else:
                result[key] = value
        return result


def main():
    with tempfile.TemporaryDirectory() as tmpdir:
        flow_dir = os.path.join(tmpdir, "flows")
        run_dir = os.path.join(tmpdir, "runs")
        os.makedirs(flow_dir, exist_ok=True)

        builder = EngineBuilder()
        builder.flow_store_path(flow_dir)
        builder.run_store_path(run_dir)
        builder.crash_recovery(False)

        # Register the custom Python node
        builder.node(UppercaseNode())
        engine = builder.build()

        # Verify it's in the catalog
        catalog = engine.node_catalog()
        custom_nodes = [n for n in catalog if n["node_type"] == "uppercase"]
        print(f"Custom 'uppercase' node registered: {len(custom_nodes) > 0}")

        # Execute an ad-hoc graph that uses the custom node
        graph = {
            "schema_version": 1,
            "id": "custom-test",
            "name": "Custom Node Test",
            "version": "1.0",
            "nodes": [
                {
                    "instance_id": "upper",
                    "node_type": "uppercase",
                    "config": {},
                }
            ],
            "edges": [],
            "tool_definitions": {},
            "metadata": {},
        }

        handle = engine.execute_graph(graph, {"greeting": "hello world", "count": 42})
        print(f"Run ID: {handle.run_id}")

        events = handle.wait()
        print(f"Received {len(events)} events")

        # Look for the node completed event to see the output
        for event in events:
            for event_type, payload in event.items():
                print(f"  [{event_type}]")
                if event_type == "NodeCompleted" and isinstance(payload, dict):
                    outputs = payload.get("outputs", {})
                    print(f"    outputs: {json.dumps(outputs)}")

        engine.shutdown()
        print("\nDone!")


if __name__ == "__main__":
    main()
