/**
 * LangGraph observability example with ClearGate.
 *
 * Demonstrates automatic capture of graph state transitions.
 *
 * Requires: Ollama running locally with qwen3:4b model.
 */

import { ChatOllama } from "@langchain/ollama";
import { HumanMessage } from "@langchain/core/messages";
import { StateGraph, MessagesAnnotation, START, END } from "@langchain/langgraph";
import { CleargateLangGraphHandler } from "cleargate";

async function chatbot(state: typeof MessagesAnnotation.State) {
  const llm = new ChatOllama({ model: "qwen3:4b" });
  const response = await llm.invoke(state.messages);
  return { messages: [response] };
}

async function main() {
  const handler = new CleargateLangGraphHandler("langgraph-example", {
    storeUrl: "sqlite://cleargate_runs.db?mode=rwc",
  });

  const graph = new StateGraph(MessagesAnnotation)
    .addNode("chatbot", chatbot)
    .addEdge(START, "chatbot")
    .addEdge("chatbot", END)
    .compile();

  console.log("Asking: Tell me a short joke.");
  const result = await graph.invoke(
    { messages: [new HumanMessage("Tell me a short joke.")] },
    { callbacks: [handler] }
  );

  const lastMessage = result.messages[result.messages.length - 1];
  console.log(`Response: ${lastMessage.content}`);

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
