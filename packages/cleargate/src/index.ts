/**
 * Cleargate — LLM observability, replay, and testing for AI agent pipelines.
 *
 * @example
 * ```ts
 * import { observe } from "cleargate";
 *
 * const session = observe("my-flow");
 * session.recordLlmCall("llm-1", requestObj, responseObj);
 * session.finish();
 * ```
 */

export {
  ObserverSession,
  AdapterSession,
  type NativeObserverSession,
  type NativeAdapterSession,
} from "./native";

export { CleargateLangChainHandler } from "./handlers/langchain";
export { CleargateLangGraphHandler } from "./handlers/langgraph";
export { CleargateSemanticKernelHandler } from "./handlers/semantic-kernel";

import { ObserverSession, type NativeObserverSession } from "./native";

/**
 * Start an observer session. Convenience function wrapping ObserverSession.start().
 */
export function observe(name: string): NativeObserverSession {
  return ObserverSession.start(name);
}
