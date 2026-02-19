/**
 * Native binding interface for the cleargate-node napi-rs crate.
 *
 * This module loads the platform-specific native binary and re-exports
 * the ObserverSession and AdapterSession classes.
 */

// The napi-rs generated binding will export these classes directly.
// This file serves as the type declaration for the native module.

export interface NativeObserverSession {
  readonly runId: string;
  recordLlmCall(nodeId: string, request: object, response: object): void;
  recordToolCall(
    toolName: string,
    inputs: object,
    outputs: object,
    durationMs: number,
  ): void;
  recordStep(stepName: string, data: object): void;
  finish(status?: string): void;
  /** Run summary: metadata, status, timing, aggregate LLM stats. Does not include the event log. */
  getRunData(): object;
  /** Full event log: LLM calls, tool invocations, steps, topology hints, etc. */
  getEvents(): object[];
}

export interface NativeAdapterSession {
  readonly runId: string;
  onEvent(event: object): void;
  finish(status?: string): void;
  /** Run summary: metadata, status, timing, aggregate LLM stats. Does not include the event log. */
  getRunData(): object;
  /** Full event log: LLM calls, tool invocations, steps, topology hints, etc. */
  getEvents(): object[];
}

export interface NativeBinding {
  ObserverSession: {
    start(name: string, storeUrl?: string): NativeObserverSession;
  };
  AdapterSession: {
    start(
      framework: string,
      name?: string,
      storeUrl?: string,
    ): NativeAdapterSession;
  };
}

let binding: NativeBinding;

try {
  // napi-rs generates a platform-specific .node file
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  binding = require("../cleargate.node");
} catch {
  throw new Error(
    "Failed to load cleargate native module. " +
      "Ensure the correct platform binary is installed.",
  );
}

export const { ObserverSession, AdapterSession } = binding;
