/**
 * Semantic Kernel handler for Cleargate observability.
 *
 * Since Semantic Kernel for JS/TS uses a different API than Python,
 * this provides manual event recording methods rather than filter hooks.
 *
 * @example
 * ```ts
 * import { CleargateSemanticKernelHandler } from "cleargate";
 *
 * const handler = new CleargateSemanticKernelHandler("my-sk-app");
 * handler.onFunctionInvoking("plugin", "function", { arg: "value" });
 * handler.onFunctionInvoked("plugin", "function", result, 150);
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

export class CleargateSemanticKernelHandler {
  private _session: NativeAdapterSession;
  private _ownsSession: boolean;
  private _functionStartTimes: Map<string, number> = new Map();

  constructor(
    sessionName = "semantic_kernel",
    options?: { session?: NativeAdapterSession },
  ) {
    if (options?.session) {
      this._session = options.session;
      this._ownsSession = false;
    } else {
      this._session = AdapterSession.start("semantic_kernel", sessionName);
      this._ownsSession = true;
    }
  }

  get runId(): string {
    return this._session.runId;
  }

  onFunctionInvoking(
    pluginName: string,
    functionName: string,
    args: Record<string, unknown> = {},
  ): void {
    const key = `${pluginName}.${functionName}`;
    this._functionStartTimes.set(key, performance.now());
    this._session.onEvent({
      callback: "function_invoking",
      plugin_name: pluginName,
      function_name: functionName,
      arguments: safeSerialize(args),
    });
  }

  onFunctionInvoked(
    pluginName: string,
    functionName: string,
    result: unknown,
    durationMs?: number,
  ): void {
    const key = `${pluginName}.${functionName}`;
    const start = this._functionStartTimes.get(key);
    this._functionStartTimes.delete(key);
    const duration =
      durationMs ?? (start ? Math.round(performance.now() - start) : 0);
    this._session.onEvent({
      callback: "function_invoked",
      plugin_name: pluginName,
      function_name: functionName,
      result: safeSerialize(result),
      duration_ms: duration,
    });
  }

  onPromptRendering(template: unknown): void {
    this._session.onEvent({
      callback: "prompt_rendering",
      template: safeSerialize(template),
    });
  }

  onPromptRendered(renderedPrompt: unknown): void {
    this._session.onEvent({
      callback: "prompt_rendered",
      rendered_prompt: safeSerialize(renderedPrompt),
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
