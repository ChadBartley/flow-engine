/**
 * Basic observability example using ObserverSession directly.
 *
 * Records LLM calls, tool calls, and steps manually.
 * Requires: Ollama running locally with qwen3:4b model.
 */

import { observe } from "cleargate";

async function callOllama(prompt: string, model = "qwen3:4b") {
  const response = await fetch("http://localhost:11434/api/chat", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      model,
      messages: [{ role: "user", content: prompt }],
      stream: false,
    }),
  });
  return response.json();
}

async function main() {
  const session = observe("basic-example");

  // Record a step
  session.recordStep("setup", JSON.stringify({ model: "qwen3:4b" }));

  // Make an LLM call and record it
  const prompt = "What is 2 + 2? Answer in one word.";
  const requestData = JSON.stringify({
    model: "qwen3:4b",
    messages: [{ role: "user", content: prompt }],
  });

  const result = await callOllama(prompt);

  const responseData = JSON.stringify({
    model: result.model ?? "qwen3:4b",
    message: result.message ?? {},
  });

  session.recordLlmCall("ollama-chat", requestData, responseData);

  // Record a tool call
  session.recordToolCall(
    "calculator",
    JSON.stringify({ expression: "2 + 2" }),
    JSON.stringify({ result: 4 }),
    5
  );

  // Finish and print results
  session.finish();

  const runData = JSON.parse(session.getRunData());
  console.log(`Run ID: ${session.runId}`);
  console.log(`Run data: ${JSON.stringify(runData, null, 2)}`);
}

main().catch(console.error);
