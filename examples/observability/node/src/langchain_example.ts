/**
 * LangChain observability example with ClearGate.
 *
 * Uses Ollama as the LLM and demonstrates automatic capture
 * of LLM calls and tool invocations via CleargateLangChainHandler.
 *
 * Requires: Ollama running locally with qwen3:4b model.
 */

import { ChatOllama } from "@langchain/ollama";
import { HumanMessage, ToolMessage, type BaseMessage } from "@langchain/core/messages";
import { tool } from "@langchain/core/tools";
import { z } from "zod";
import { CleargateLangChainHandler } from "cleargate";

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

const TOOLS = [addTool];
const TOOLS_BY_NAME: Record<string, typeof addTool> = Object.fromEntries(
  TOOLS.map((t) => [t.name, t])
);

async function main() {
  const handler = new CleargateLangChainHandler("langchain-example", {
    storeUrl: "sqlite://cleargate_runs.db?mode=rwc",
  });

  const llm = new ChatOllama({ model: "qwen3:4b", callbacks: [handler] });
  const llmWithTools = llm.bindTools(TOOLS);

  const messages: BaseMessage[] = [
    new HumanMessage("What is 3 + 4? Use the add tool."),
  ];

  console.log("Asking: What is 3 + 4?");

  // Agent loop: call LLM, execute any tool calls, repeat until done
  while (true) {
    const response = await llmWithTools.invoke(messages, {
      callbacks: [handler],
    });
    messages.push(response);

    const toolCalls = response.tool_calls ?? [];
    if (toolCalls.length === 0) {
      console.log(`Response: ${response.content}`);
      break;
    }

    for (const tc of toolCalls) {
      const toolFn = TOOLS_BY_NAME[tc.name];
      const result = await toolFn.invoke(tc.args, {
        callbacks: [handler],
      });
      messages.push(
        new ToolMessage({ content: String(result), tool_call_id: tc.id! })
      );
    }
  }

  handler.finish();

  // Print captured events — getEvents() returns object[], no JSON.parse needed
  const events = handler.getEvents();
  console.log(`\nCaptured ${events.length} events`);
  console.log(`Run ID: ${handler.runId}`);

  // Summarise events by type
  const counts: Record<string, number> = {};
  for (const evt of events) {
    const evtObj = evt as Record<string, unknown>;
    const type = (evtObj.type as string) ?? "unknown";
    counts[type] = (counts[type] ?? 0) + 1;
  }
  console.log("\nEvent summary:");
  for (const [type, count] of Object.entries(counts)) {
    console.log(`  ${type}: ${count}`);
  }

  // Print each event
  for (let i = 0; i < events.length; i++) {
    console.log(`\n--- Event ${i + 1} ---`);
    console.log(JSON.stringify(events[i], null, 2));
  }

  console.log("\nRun data persisted to cleargate_runs.db");
}

main().catch(console.error);
