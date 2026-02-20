/**
 * Engine orchestration types for the Cleargate Node.js SDK.
 *
 * Provides type-safe wrappers around the native `NapiEngineBuilder`,
 * `NapiEngine`, and `NapiExecutionHandle` classes.
 */

// ---------------------------------------------------------------------------
// Native interfaces (mirror the napi-rs generated classes)
// ---------------------------------------------------------------------------

export interface NativeExecutionHandle {
  readonly runId: string;
  nextEvent(): object | null;
  cancel(): void;
  waitForCompletion(): object[];
}

export interface NativeEngine {
  execute(flowId: string, inputs?: object): NativeExecutionHandle;
  executeGraph(graphJson: object, inputs?: object): NativeExecutionHandle;
  provideInput(runId: string, nodeId: string, input: object): void;
  nodeCatalog(): object[];
  shutdown(): void;
}

export interface NativeEngineBuilder {
  flowStorePath(path: string): void;
  runStorePath(path: string): void;
  runStoreUrl(url: string): void;
  maxTraversals(value: number): void;
  crashRecovery(enabled: boolean): void;
  build(): NativeEngine;
}

// ---------------------------------------------------------------------------
// WriteEvent discriminated union
// ---------------------------------------------------------------------------

/** Base fields present on all write events. */
interface WriteEventBase {
  type: string;
  run_id: string;
  seq: number;
  timestamp: string;
}

export interface RunStartedEvent extends WriteEventBase {
  type: "RunStarted";
  flow_id: string;
  version_id: string;
}

export interface RunCompletedEvent extends WriteEventBase {
  type: "RunCompleted";
  status: string;
}

export interface NodeStartedEvent extends WriteEventBase {
  type: "NodeStarted";
  node_instance_id: string;
  node_type: string;
}

export interface NodeCompletedEvent extends WriteEventBase {
  type: "NodeCompleted";
  node_instance_id: string;
  outputs: object;
}

export interface NodeFailedEvent extends WriteEventBase {
  type: "NodeFailed";
  node_instance_id: string;
  error: string;
}

export interface LlmCallEvent extends WriteEventBase {
  type: "LlmCall";
  node_instance_id: string;
  request: object;
  response: object;
}

export interface CustomEvent extends WriteEventBase {
  type: "Custom";
  name: string;
  data: object;
}

/** Union of all known event types. */
export type WriteEvent =
  | RunStartedEvent
  | RunCompletedEvent
  | NodeStartedEvent
  | NodeCompletedEvent
  | NodeFailedEvent
  | LlmCallEvent
  | CustomEvent
  | (WriteEventBase & Record<string, unknown>);

// ---------------------------------------------------------------------------
// High-level wrappers
// ---------------------------------------------------------------------------

/**
 * Async generator that yields events from an execution handle.
 *
 * ```ts
 * for await (const event of streamEvents(handle)) {
 *   console.log(event.type, event);
 * }
 * ```
 */
export async function* streamEvents(
  handle: NativeExecutionHandle,
): AsyncGenerator<WriteEvent> {
  while (true) {
    const event = handle.nextEvent() as WriteEvent | null;
    if (event === null) break;
    yield event;
  }
}

// ---------------------------------------------------------------------------
// Node handler / LLM provider interfaces (for future bridging)
// ---------------------------------------------------------------------------

/** Interface for a custom node handler (future: register via EngineBuilder). */
export interface NodeHandler {
  meta(): {
    node_type: string;
    label: string;
    category: string;
    inputs: object[];
    outputs: object[];
    config_schema: object;
  };
  run(
    inputs: object,
    config: object,
    context: object,
  ): object | Promise<object>;
}

/** Interface for a custom LLM provider (future: register via EngineBuilder). */
export interface LlmProvider {
  readonly name: string;
  complete(request: object): object | Promise<object>;
}
