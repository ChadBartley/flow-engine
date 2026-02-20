#!/usr/bin/env python3
"""Streaming Events

Demonstrates real-time event streaming using the iterator protocol:
  for event in handle:
      process(event)
"""

import os
import shutil
import tempfile
import time

from cleargate import EngineBuilder


def main():
    flow_src = os.path.join(os.path.dirname(__file__), "..", "flows", "echo-flow.json")
    with tempfile.TemporaryDirectory() as tmpdir:
        flow_dir = os.path.join(tmpdir, "flows")
        run_dir = os.path.join(tmpdir, "runs")

        flow_dest = os.path.join(flow_dir, "echo-flow")
        os.makedirs(flow_dest, exist_ok=True)
        shutil.copy(flow_src, os.path.join(flow_dest, "flow.json"))

        builder = EngineBuilder()
        builder.flow_store_path(flow_dir)
        builder.run_store_path(run_dir)
        builder.crash_recovery(False)
        engine = builder.build()

        print("Executing echo-flow with streaming events...\n")
        handle = engine.execute("echo-flow", {"message": "Stream test"})

        start = time.monotonic()
        for event in handle:
            elapsed = time.monotonic() - start
            # Each event is a dict with a single key (the event variant)
            for event_type, payload in event.items():
                print(f"  [{elapsed:.3f}s] {event_type}")
                if isinstance(payload, dict) and "node_instance_id" in payload:
                    print(f"           node: {payload['node_instance_id']}")

        print(f"\nAll events received in {time.monotonic() - start:.3f}s")
        engine.shutdown()


if __name__ == "__main__":
    main()
