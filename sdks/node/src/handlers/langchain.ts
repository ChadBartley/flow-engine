/**
 * LangChain.js callback handler for Cleargate observability.
 *
 * Extends `BaseCallbackHandler` so LangChain recognises this as a
 * first-class callback and dispatches events to it automatically.
 *
 * @example
 * ```ts
 * import { CleargateLangChainHandler } from "cleargate";
 *
 * const handler = new CleargateLangChainHandler("my-chain");
 * const result = await model.invoke("Hello", { callbacks: [handler] });
 * handler.finish();
 * console.log(handler.getRunData());
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

/** Map LangChain message type names to standard OpenAI-style roles. */
const ROLE_MAP: Record<string, string> = {
  human: "user",
  ai: "assistant",
  system: "system",
  tool: "tool",
  function: "function",
  chat: "assistant",
};

/** Convert LangChain message objects to `{role, content, ...}` dicts. */
function messagesToDicts(
  messageLists: BaseMessage[][],
): Record<string, unknown>[] {
  const result: Record<string, unknown>[] = [];
  for (const msgList of messageLists) {
    for (const msg of msgList) {
      const lcType = msg.getType();
      const role = ROLE_MAP[lcType] ?? lcType;
      const entry: Record<string, unknown> = {
        role,
        content: msg.content,
      };
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

/**
 * Extract structured data from a LangChain `LLMResult`.
 *
 * `LLMResult` is not directly JSON-serializable in a useful way, so we
 * pull the fields the Rust adapter expects: `generations`, `llm_output`.
 */
function serializeLlmResult(result: LLMResult): Record<string, unknown> {
  const out: Record<string, unknown> = {};

  if (result.generations) {
    const serializedGens = result.generations.map((genList) =>
      genList.map((gen) => {
        const entry: Record<string, unknown> = { text: gen.text ?? "" };
        if (gen.generationInfo) {
          entry.generation_info = safeSerialize(gen.generationInfo);
        }
        // Chat generations have a message property with response_metadata and usage_metadata
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
    out.generations = serializedGens;
  }

  if (result.llmOutput) {
    out.llm_output = safeSerialize(result.llmOutput);
  }

  return out;
}

/**
 * Extract model name and provider from LangChain serialized data and invocation params.
 *
 * JS LangChain providers (e.g. ChatOllama) serialize as `type: "not_implemented"`
 * without `kwargs` or `repr`, so model/provider can't be extracted from `Serialized`
 * alone. The `extraParams.invocation_params` object (populated by the provider's
 * `invocationParams()` method) reliably contains the model name.
 */
function extractModelProvider(
  llm: Serialized,
  extraParams?: Record<string, unknown>,
): { model: string; provider: string } {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const invParams = (extraParams as any)?.invocation_params as
    | Record<string, unknown>
    | undefined;

  // Model: prefer invocation_params.model, then kwargs paths, then last id element
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

  // Provider: strip langchain_ / langchain- prefix from first id element
  const ids = (llm as { id?: string[] }).id;
  const rawProvider =
    Array.isArray(ids) && ids.length > 0 ? ids[0] : "langchain";
  const provider =
    rawProvider.replace(/^langchain[_-]/, "") ||
    // If after stripping we still have "langchain", it's the base package — keep it
    "langchain";

  return { model, provider };
}

export class CleargateLangChainHandler extends BaseCallbackHandler {
  name = "CleargateLangChainHandler";

  private _session: NativeAdapterSession;
  private _ownsSession: boolean;
  private _llmStartTimes: Map<string, number> = new Map();
  /** Tracks runIds that already received a handleChatModelStart to suppress duplicate handleLLMStart. */
  private _chatModelStarted: Set<string> = new Set();
  /** Maps tool runId to tool name from handleToolStart for use in handleToolEnd. */
  private _pendingToolNames: Map<string, string> = new Map();

  constructor(
    sessionName = "langchain",
    options?: { session?: NativeAdapterSession; storeUrl?: string },
  ) {
    super();
    if (options?.session) {
      this._session = options.session;
      this._ownsSession = false;
    } else {
      this._session = AdapterSession.start(
        "langchain",
        sessionName,
        options?.storeUrl,
      );
      this._ownsSession = true;
    }
  }

  get runId(): string {
    return this._session.runId;
  }

  /**
   * Called for non-chat LLMs with flat prompt strings.
   * Skipped when handleChatModelStart already fired for the same runId.
   */
  handleLLMStart(
    llm: Serialized,
    prompts: string[],
    runId: string,
    _parentRunId?: string,
    extraParams?: Record<string, unknown>,
  ): void {
    // Chat models fire both handleChatModelStart and handleLLMStart — skip the duplicate
    if (this._chatModelStarted.has(runId)) {
      return;
    }
    this._llmStartTimes.set(runId, performance.now());
    const { model, provider } = extractModelProvider(llm, extraParams);
    this._session.onEvent({
      callback: "on_llm_start",
      run_id: runId,
      serialized: safeSerialize(llm),
      prompts,
      model,
      provider,
    });
  }

  /**
   * Called for chat models with structured message objects.
   * Produces the same `on_llm_start` event but with `{role, content}` messages
   * instead of flat strings.
   */
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
      run_id: runId,
      response: serializeLlmResult(output),
      duration_ms: durationMs,
    });
  }

  handleToolStart(tool: Serialized, input: string, runId: string): void {
    const toolName = (tool.name ?? "unknown") as string;
    this._pendingToolNames.set(runId, toolName);
    this._session.onEvent({
      callback: "on_tool_start",
      run_id: runId,
      serialized: safeSerialize(tool),
      input_str: input,
    });
  }

  handleToolEnd(output: unknown, runId: string): void {
    const toolName = this._pendingToolNames.get(runId) ?? "unknown";
    this._pendingToolNames.delete(runId);
    this._session.onEvent({
      callback: "on_tool_end",
      run_id: runId,
      tool_name: toolName,
      output: safeSerialize(output),
    });
  }

  handleChainStart(
    chain: Serialized,
    inputs: ChainValues,
    runId: string,
  ): void {
    this._session.onEvent({
      callback: "on_chain_start",
      run_id: runId,
      serialized: safeSerialize(chain),
      inputs: safeSerialize(inputs),
    });
  }

  handleChainEnd(outputs: ChainValues, runId: string): void {
    this._session.onEvent({
      callback: "on_chain_end",
      run_id: runId,
      outputs: safeSerialize(outputs),
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
