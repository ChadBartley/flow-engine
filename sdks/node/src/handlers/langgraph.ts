/**
 * LangGraph.js callback handler for Cleargate observability.
 *
 * Extends `BaseCallbackHandler` with node/edge tracking for graph execution.
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

import { BaseCallbackHandler } from "@langchain/core/callbacks/base";
import type { Serialized } from "@langchain/core/load/serializable";
import type { LLMResult } from "@langchain/core/outputs";
import type { ChainValues } from "@langchain/core/utils/types";
import type { BaseMessage } from "@langchain/core/messages";
import { AdapterSession, type NativeAdapterSession } from "../native";

function safeSerialize(obj: unknown): unknown {
  try {
    return JSON.parse(JSON.stringify(obj));
  } catch {
    return String(obj);
  }
}

const ROLE_MAP: Record<string, string> = {
  human: "user",
  ai: "assistant",
  system: "system",
  tool: "tool",
  function: "function",
  chat: "assistant",
};

function messagesToDicts(
  messageLists: BaseMessage[][],
): Record<string, unknown>[] {
  const result: Record<string, unknown>[] = [];
  for (const msgList of messageLists) {
    for (const msg of msgList) {
      const lcType = msg.getType();
      const role = ROLE_MAP[lcType] ?? lcType;
      const entry: Record<string, unknown> = { role, content: msg.content };
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const raw = msg as any;
      if (raw.tool_calls?.length) {
        entry.tool_calls = raw.tool_calls.map(
          (tc: Record<string, unknown>) => ({
            id: tc.id ?? "",
            tool_name: tc.name ?? "",
            arguments: safeSerialize(tc.args ?? {}),
          }),
        );
      }
      if (raw.tool_call_id) {
        entry.tool_call_id = raw.tool_call_id;
      }
      result.push(entry);
    }
  }
  return result;
}

function serializeLlmResult(result: LLMResult): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (result.generations) {
    out.generations = result.generations.map((genList) =>
      genList.map((gen) => {
        const entry: Record<string, unknown> = { text: gen.text ?? "" };
        if (gen.generationInfo) {
          entry.generation_info = safeSerialize(gen.generationInfo);
        }
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const raw = gen as any;
        if (raw.message) {
          if (raw.message.response_metadata) {
            entry.generation_info = safeSerialize(
              raw.message.response_metadata,
            );
          }
          if (raw.message.usage_metadata) {
            entry.usage_metadata = safeSerialize(raw.message.usage_metadata);
          }
        }
        return entry;
      }),
    );
  }
  if (result.llmOutput) {
    out.llm_output = safeSerialize(result.llmOutput);
  }
  return out;
}

/**
 * Extract model name and provider from LangChain serialized data and invocation params.
 * See langchain.ts for detailed rationale.
 */
function extractModelProvider(
  llm: Serialized,
  extraParams?: Record<string, unknown>,
): { model: string; provider: string } {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const invParams = (extraParams as any)?.invocation_params as
    | Record<string, unknown>
    | undefined;

  const model =
    (invParams?.model as string) ??
    (invParams?.model_name as string) ??
    (() => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const s = llm as any;
      const kw = s.kwargs;
      return (kw?.model_name ??
        kw?.model ??
        kw?.kwargs?.model_name ??
        kw?.kwargs?.model) as string | undefined;
    })() ??
    (() => {
      const ids = (llm as { id?: string[] }).id;
      return Array.isArray(ids) ? ids[ids.length - 1] : undefined;
    })() ??
    "unknown";

  const ids = (llm as { id?: string[] }).id;
  const rawProvider =
    Array.isArray(ids) && ids.length > 0 ? ids[0] : "langchain";
  const provider = rawProvider.replace(/^langchain[_-]/, "") || "langchain";

  return { model, provider };
}

export class CleargateLangGraphHandler extends BaseCallbackHandler {
  name = "CleargateLangGraphHandler";

  private _session: NativeAdapterSession;
  private _ownsSession: boolean;
  private _llmStartTimes: Map<string, number> = new Map();
  private _chatModelStarted: Set<string> = new Set();
  private _currentNode: string | null = null;

  constructor(
    sessionName = "langgraph",
    options?: { session?: NativeAdapterSession; storeUrl?: string },
  ) {
    super();
    if (options?.session) {
      this._session = options.session;
      this._ownsSession = false;
    } else {
      this._session = AdapterSession.start(
        "langgraph",
        sessionName,
        options?.storeUrl,
      );
      this._ownsSession = true;
    }
  }

  get runId(): string {
    return this._session.runId;
  }

  handleChainStart(
    chain: Serialized,
    inputs: ChainValues,
    runId: string,
    _parentRunId?: string,
    tags?: string[],
  ): void {
    const idArr = chain.id;
    const name =
      (Array.isArray(idArr) ? idArr[idArr.length - 1] : null) ?? "chain";

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
    outputs: ChainValues,
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
    llm: Serialized,
    prompts: string[],
    runId: string,
    _parentRunId?: string,
    extraParams?: Record<string, unknown>,
  ): void {
    if (this._chatModelStarted.has(runId)) {
      return;
    }
    this._llmStartTimes.set(runId, performance.now());
    const { model, provider } = extractModelProvider(llm, extraParams);
    this._session.onEvent({
      callback: "on_llm_start",
      node: this._currentNode ?? "llm",
      run_id: runId,
      serialized: safeSerialize(llm),
      prompts,
      model,
      provider,
    });
  }

  handleChatModelStart(
    llm: Serialized,
    messages: BaseMessage[][],
    runId: string,
    _parentRunId?: string,
    extraParams?: Record<string, unknown>,
  ): void {
    this._chatModelStarted.add(runId);
    this._llmStartTimes.set(runId, performance.now());
    const { model, provider } = extractModelProvider(llm, extraParams);
    this._session.onEvent({
      callback: "on_llm_start",
      node: this._currentNode ?? "llm",
      run_id: runId,
      serialized: safeSerialize(llm),
      prompts: messagesToDicts(messages),
      model,
      provider,
    });
  }

  handleLLMEnd(output: LLMResult, runId: string): void {
    this._chatModelStarted.delete(runId);
    const start = this._llmStartTimes.get(runId);
    this._llmStartTimes.delete(runId);
    const durationMs = start ? Math.round(performance.now() - start) : 0;
    this._session.onEvent({
      callback: "on_llm_end",
      node: this._currentNode ?? "llm",
      run_id: runId,
      response: serializeLlmResult(output),
      duration_ms: durationMs,
    });
  }

  handleToolStart(tool: Serialized, input: string, runId: string): void {
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
