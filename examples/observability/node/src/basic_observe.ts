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
  const session = observe("basic-example", "sqlite://cleargate_runs.db?mode=rwc");

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

  // getRunData() returns an object, no JSON.parse needed
  const runData = session.getRunData();
  console.log(`Run ID: ${session.runId}`);
  console.log(`Run data: ${JSON.stringify(runData, null, 2)}`);

  // Print captured events
  const events = session.getEvents();
  console.log(`\nCaptured ${events.length} events`);

  for (let i = 0; i < events.length; i++) {
    const evt = events[i] as Record<string, unknown>;
    console.log(`\n--- Event ${i + 1} ---`);
    console.log(`  Type: ${evt.type ?? "unknown"}`);
    console.log(JSON.stringify(evt, null, 2));
  }

  console.log("\nRun data persisted to cleargate_runs.db");
}

main().catch(console.error);
