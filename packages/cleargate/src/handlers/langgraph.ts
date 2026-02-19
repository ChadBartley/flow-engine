/**
 * LangGraph.js callback handler for Cleargate observability.
 *
 * Extends LangChain callbacks with node/edge tracking for graph execution.
 *
 * @example
 * ```ts
 * import { CleargateLangGraphHandler } from "cleargate";
 *
 * const handler = new CleargateLangGraphHandler("my-graph");
 * const result = await app.invoke({ messages: [...] }, { callbacks: [handler] });
 * handler.finish();
 * ```
 */

import { AdapterSession, type NativeAdapterSession } from "../native";

function safeSerialize(obj: unknown): unknown {
  try {
    return JSON.parse(JSON.stringify(obj));
  } catch {
    return String(obj);
  }
}

export class CleargateLangGraphHandler {
  private _session: NativeAdapterSession;
  private _ownsSession: boolean;
  private _llmStartTimes: Map<string, number> = new Map();
  private _currentNode: string | null = null;

  constructor(
    sessionName = "langgraph",
    options?: { session?: NativeAdapterSession },
  ) {
    if (options?.session) {
      this._session = options.session;
      this._ownsSession = false;
    } else {
      this._session = AdapterSession.start("langgraph", sessionName);
      this._ownsSession = true;
    }
  }

  get runId(): string {
    return this._session.runId;
  }

  handleChainStart(
    chain: Record<string, unknown>,
    inputs: Record<string, unknown>,
    runId: string,
    _parentRunId?: string,
    tags?: string[],
  ): void {
    const idArr = chain.id as string[] | undefined;
    const name =
      (Array.isArray(idArr) ? idArr[idArr.length - 1] : null) ??
      (chain.name as string) ??
      "chain";

    let nodeName: string | null = null;
    for (const tag of tags ?? []) {
      if (typeof tag === "string" && tag.startsWith("graph:step:")) {
        nodeName = tag.split(":").slice(2).join(":");
        break;
      }
    }

    if (nodeName) {
      if (this._currentNode && this._currentNode !== nodeName) {
        this._session.onEvent({
          callback: "edge",
          from: this._currentNode,
          to: nodeName,
        });
      }
      this._currentNode = nodeName;
      this._session.onEvent({
        callback: "node_start",
        node: nodeName,
        state: safeSerialize(inputs),
      });
    } else {
      this._session.onEvent({
        callback: "node_start",
        node: name,
        state: safeSerialize(inputs),
      });
    }
  }

  handleChainEnd(
    outputs: Record<string, unknown>,
    runId: string,
    _parentRunId?: string,
    tags?: string[],
  ): void {
    let nodeName: string | null = null;
    for (const tag of tags ?? []) {
      if (typeof tag === "string" && tag.startsWith("graph:step:")) {
        nodeName = tag.split(":").slice(2).join(":");
        break;
      }
    }

    this._session.onEvent({
      callback: "node_end",
      node: nodeName ?? this._currentNode ?? "chain",
      state: safeSerialize(outputs),
    });
  }

  handleLLMStart(
    llm: Record<string, unknown>,
    prompts: string[],
    runId: string,
  ): void {
    this._llmStartTimes.set(runId, performance.now());
    const kwargs = (llm.kwargs ?? {}) as Record<string, unknown>;
    const model = (kwargs.model_name ?? kwargs.model ?? "unknown") as string;
    this._session.onEvent({
      callback: "on_llm_start",
      node: this._currentNode ?? "llm",
      request: { model, messages: prompts },
    });
  }

  handleLLMEnd(output: Record<string, unknown>, runId: string): void {
    const start = this._llmStartTimes.get(runId);
    this._llmStartTimes.delete(runId);
    const durationMs = start ? Math.round(performance.now() - start) : 0;
    this._session.onEvent({
      callback: "on_llm_end",
      node: this._currentNode ?? "llm",
      response: safeSerialize(output),
      duration_ms: durationMs,
    });
  }

  handleToolStart(
    tool: Record<string, unknown>,
    input: string,
    runId: string,
  ): void {
    const toolName = (tool.name ?? "unknown") as string;
    this._session.onEvent({
      callback: "node_start",
      node: toolName,
      state: { input },
    });
  }

  handleToolEnd(output: unknown, _runId: string): void {
    this._session.onEvent({
      callback: "node_end",
      node: "tool",
      state: { output: safeSerialize(output) },
    });
  }

  emitStateUpdate(node: string, updates: Record<string, unknown>): void {
    this._session.onEvent({
      callback: "state_update",
      node,
      updates: safeSerialize(updates),
    });
  }

  finish(status = "completed"): void {
    if (this._ownsSession) {
      this._session.finish(status);
    }
  }

  /** Run summary: metadata, status, timing, aggregate LLM stats. Use with getEvents() for the full picture. */
  getRunData(): object {
    return this._session.getRunData();
  }

  /** Full event log (LLM calls, tool invocations, steps, etc.) for DiffEngine and ReplayEngine. */
  getEvents(): object[] {
    return this._session.getEvents();
  }
}
