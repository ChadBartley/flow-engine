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
import { observe, CleargateLangGraphHandler } from "cleargate";

async function chatbot(state: typeof MessagesAnnotation.State) {
  const llm = new ChatOllama({ model: "qwen3:4b" });
  const response = await llm.invoke(state.messages);
  return { messages: [response] };
}

async function main() {
  const session = observe("langgraph-example");
  const handler = new CleargateLangGraphHandler(session);

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

  session.finish();

  const events = JSON.parse(session.getEvents());
  console.log(`\nCaptured ${events.length} events`);
  console.log(`Run ID: ${session.runId}`);
}

main().catch(console.error);
