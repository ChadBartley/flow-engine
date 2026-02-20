#!/usr/bin/env python3
"""Basic Flow Execution

Demonstrates the minimal engine setup:
1. Create an EngineBuilder
2. Point it at a flow store directory containing JSON flow definitions
3. Build the engine
4. Execute a flow and collect all events
"""

import os
import shutil
import tempfile

from cleargate import EngineBuilder


def main():
    # Copy the echo flow into a temp directory so the engine can find it.
    flow_src = os.path.join(os.path.dirname(__file__), "..", "flows", "echo-flow.json")
    with tempfile.TemporaryDirectory() as tmpdir:
        flow_dir = os.path.join(tmpdir, "flows")
        run_dir = os.path.join(tmpdir, "runs")
        os.makedirs(flow_dir, exist_ok=True)

        # The engine expects flows in <flow_store>/<flow_id>/flow.json
        flow_dest = os.path.join(flow_dir, "echo-flow")
        os.makedirs(flow_dest, exist_ok=True)
        shutil.copy(flow_src, os.path.join(flow_dest, "flow.json"))

        # Build the engine
        builder = EngineBuilder()
        builder.flow_store_path(flow_dir)
        builder.run_store_path(run_dir)
        builder.crash_recovery(False)
        engine = builder.build()

        print("Engine built. Registered nodes:")
        catalog = engine.node_catalog()
        for node in catalog:
            print(f"  - {node['node_type']} ({node['category']})")

        # Execute the echo flow
        print("\nExecuting echo-flow...")
        handle = engine.execute("echo-flow", {"message": "Hello from Python!"})
        print(f"Run ID: {handle.run_id}")

        # Collect all events
        events = handle.wait()
        print(f"\nReceived {len(events)} events:")
        for event in events:
            # WriteEvent is a tagged enum — the type is the key
            if isinstance(event, dict):
                for key in event:
                    print(f"  [{key}]")
                    break

        engine.shutdown()
        print("\nDone!")


if __name__ == "__main__":
    main()
