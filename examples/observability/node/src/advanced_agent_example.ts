/**
 * Advanced agent observability example with ClearGate.
 *
 * Demonstrates a multi-tool agent with full event capture and persistent
 * storage — showing the complete value of ClearGate observability for
 * debugging and understanding agent behavior.
 *
 * Requires: Ollama running locally with ministral-3:3b model.
 */

import { ChatOllama } from "@langchain/ollama";
import {
  HumanMessage,
  ToolMessage,
  type BaseMessage,
} from "@langchain/core/messages";
import { tool } from "@langchain/core/tools";
import { z } from "zod";
import { CleargateLangChainHandler } from "cleargate";

const getWeather = tool(
  async ({ city }: { city: string }) => {
    const weather: Record<string, string> = {
      london: "Cloudy, 12°C, 80% humidity",
      paris: "Sunny, 18°C, 45% humidity",
      tokyo: "Rainy, 15°C, 90% humidity",
      "new york": "Partly cloudy, 22°C, 55% humidity",
    };
    return (
      weather[city.toLowerCase()] ?? `Weather data not available for ${city}`
    );
  },
  {
    name: "get_weather",
    description: "Get the current weather for a city",
    schema: z.object({ city: z.string() }),
  },
);

const calculate = tool(
  async ({ expression }: { expression: string }) => {
    // Only allow digits and arithmetic operators
    if (!/^[\d\s+\-*/().]+$/.test(expression)) {
      return `Invalid expression: ${expression}`;
    }
    try {
      const result = Function(`"use strict"; return (${expression})`)();
      return String(result);
    } catch (e) {
      return `Error: ${e}`;
    }
  },
  {
    name: "calculate",
    description: "Evaluate a simple arithmetic expression",
    schema: z.object({ expression: z.string() }),
  },
);

const unitConvert = tool(
  async ({
    value,
    fromUnit,
    toUnit,
  }: {
    value: number;
    fromUnit: string;
    toUnit: string;
  }) => {
    const conversions: Record<string, (v: number) => number> = {
      "celsius->fahrenheit": (v) => (v * 9) / 5 + 32,
      "fahrenheit->celsius": (v) => ((v - 32) * 5) / 9,
      "celsius->kelvin": (v) => v + 273.15,
      "kelvin->celsius": (v) => v - 273.15,
    };
    const key = `${fromUnit.toLowerCase()}->${toUnit.toLowerCase()}`;
    const fn = conversions[key];
    if (!fn) return `Unsupported conversion: ${fromUnit} -> ${toUnit}`;
    return `${value} ${fromUnit} = ${fn(value).toFixed(2)} ${toUnit}`;
  },
  {
    name: "unit_convert",
    description:
      "Convert between temperature units (celsius, fahrenheit, kelvin)",
    schema: z.object({
      value: z.number(),
      fromUnit: z.string(),
      toUnit: z.string(),
    }),
  },
);

const TOOLS = [getWeather, calculate, unitConvert];
const TOOLS_BY_NAME: Record<string, (typeof TOOLS)[number]> =
  Object.fromEntries(TOOLS.map((t) => [t.name, t]));

function printEventSummary(events: object[]): void {
  const counts: Record<string, number> = {};
  for (const evt of events) {
    const evtObj = evt as Record<string, unknown>;
    const type = (evtObj.event_type as string) ?? (evtObj.type as string) ?? "unknown";
    counts[type] = (counts[type] ?? 0) + 1;
  }

  console.log("\n" + "=".repeat(60));
  console.log("EVENT SUMMARY");
  console.log("=".repeat(60));
  console.log(`Total events: ${events.length}`);
  for (const [type, count] of Object.entries(counts).sort()) {
    console.log(`  ${type}: ${count}`);
  }

  console.log("\n" + "-".repeat(60));
  console.log("EVENT DETAILS");
  console.log("-".repeat(60));

  for (let i = 0; i < events.length; i++) {
    const evt = events[i] as Record<string, unknown>;
    console.log(`\n--- Event ${i + 1}: ${evt.event_type ?? evt.type ?? "unknown"} ---`);
    console.log(JSON.stringify(evt, null, 2));
  }
}

async function main() {
  const handler = new CleargateLangChainHandler("advanced-agent", {
    storeUrl: "sqlite://cleargate_runs.db?mode=rwc",
  });

  const llm = new ChatOllama({ model: "ministral-3:3b" });
  const llmWithTools = llm.bindTools(TOOLS);

  const query =
    "What's the weather in Paris? " +
    "Convert the temperature from Celsius to Fahrenheit, " +
    "then calculate 18 * 9 / 5 + 32 to verify.";

  const messages: BaseMessage[] = [new HumanMessage(query)];

  console.log(`Query: ${query}`);
  console.log("-".repeat(60));

  let iteration = 0;
  while (true) {
    iteration++;
    console.log(`\n[Agent iteration ${iteration}] Thinking...`);

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
      console.log(`  Tool call: ${tc.name}(${JSON.stringify(tc.args)})`);
      const toolFn = TOOLS_BY_NAME[tc.name];
      const result = await toolFn.invoke(tc.args, {
        callbacks: [handler],
      });
      console.log(`  Result: ${result}`);
      messages.push(
        new ToolMessage({ content: String(result), tool_call_id: tc.id! }),
      );
    }
  }

  handler.finish();

  // Print full event capture
  const events = handler.getEvents();
  printEventSummary(events);

  // Print run data
  const runData = handler.getRunData();
  console.log("\n" + "=".repeat(60));
  console.log("RUN DATA");
  console.log("=".repeat(60));
  console.log(JSON.stringify(runData, null, 2));

  console.log(`\nRun ID: ${handler.runId}`);
  console.log("Run data persisted to cleargate_runs.db");
}

main().catch(console.error);
