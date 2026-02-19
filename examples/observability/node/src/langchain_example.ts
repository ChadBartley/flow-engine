/**
 * LangChain observability example with ClearGate.
 *
 * Uses Ollama as the LLM and demonstrates automatic capture
 * of LLM calls and tool invocations via CleargateLangChainHandler.
 *
 * Requires: Ollama running locally with qwen3:4b model.
 */

import { ChatOllama } from "@langchain/ollama";
import { HumanMessage } from "@langchain/core/messages";
import { tool } from "@langchain/core/tools";
import { z } from "zod";
import { observe, CleargateLangChainHandler } from "cleargate";

const addTool = tool(
  async ({ a, b }: { a: number; b: number }) => {
    return (a + b).toString();
  },
  {
    name: "add",
    description: "Add two numbers together",
    schema: z.object({ a: z.number(), b: z.number() }),
  }
);

async function main() {
  const session = observe("langchain-example");
  const handler = new CleargateLangChainHandler(session);

  const llm = new ChatOllama({ model: "qwen3:4b", callbacks: [handler] });
  const llmWithTools = llm.bindTools([addTool]);

  console.log("Asking: What is 3 + 4?");
  const response = await llmWithTools.invoke(
    [new HumanMessage("What is 3 + 4? Use the add tool.")],
    { callbacks: [handler] }
  );
  console.log(`Response: ${response.content}`);

  session.finish();

  const events = JSON.parse(session.getEvents());
  console.log(`\nCaptured ${events.length} events`);
  console.log(`Run ID: ${session.runId}`);
}

main().catch(console.error);
