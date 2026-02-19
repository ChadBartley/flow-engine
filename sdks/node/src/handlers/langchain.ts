/**
 * LangChain.js callback handler for Cleargate observability.
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

import { AdapterSession, type NativeAdapterSession } from "../native";

function safeSerialize(obj: unknown): unknown {
  try {
    return JSON.parse(JSON.stringify(obj));
  } catch {
    return String(obj);
  }
}

export class CleargateLangChainHandler {
  private _session: NativeAdapterSession;
  private _ownsSession: boolean;
  private _llmStartTimes: Map<string, number> = new Map();

  constructor(
    sessionName = "langchain",
    options?: { session?: NativeAdapterSession; storeUrl?: string },
  ) {
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
   * LangChain.js handleLLMStart callback.
   */
  handleLLMStart(
    llm: Record<string, unknown>,
    prompts: string[],
    runId: string,
  ): void {
    this._llmStartTimes.set(runId, performance.now());
    this._session.onEvent({
      callback: "on_llm_start",
      run_id: runId,
      serialized: safeSerialize(llm),
      prompts,
    });
  }

  /**
   * LangChain.js handleLLMEnd callback.
   */
  handleLLMEnd(output: Record<string, unknown>, runId: string): void {
    const start = this._llmStartTimes.get(runId);
    this._llmStartTimes.delete(runId);
    const durationMs = start ? Math.round(performance.now() - start) : 0;
    this._session.onEvent({
      callback: "on_llm_end",
      run_id: runId,
      response: safeSerialize(output),
      duration_ms: durationMs,
    });
  }

  /**
   * LangChain.js handleToolStart callback.
   */
  handleToolStart(
    tool: Record<string, unknown>,
    input: string,
    runId: string,
  ): void {
    this._session.onEvent({
      callback: "on_tool_start",
      run_id: runId,
      serialized: safeSerialize(tool),
      input_str: input,
    });
  }

  /**
   * LangChain.js handleToolEnd callback.
   */
  handleToolEnd(output: unknown, runId: string): void {
    this._session.onEvent({
      callback: "on_tool_end",
      run_id: runId,
      output: safeSerialize(output),
    });
  }

  /**
   * LangChain.js handleChainStart callback.
   */
  handleChainStart(
    chain: Record<string, unknown>,
    inputs: Record<string, unknown>,
    runId: string,
  ): void {
    this._session.onEvent({
      callback: "on_chain_start",
      run_id: runId,
      serialized: safeSerialize(chain),
      inputs: safeSerialize(inputs),
    });
  }

  /**
   * LangChain.js handleChainEnd callback.
   */
  handleChainEnd(outputs: Record<string, unknown>, runId: string): void {
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
