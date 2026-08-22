import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.registerProvider("e2e-openai", {
    name: "E2E OpenAI Completions",
    baseUrl: process.env.OPENAI_COMPLETIONS_BASE_URL!,
    apiKey: "$OPENAI_COMPLETIONS_API_KEY",
    api: "openai-completions",
    models: [{
      id: process.env.OPENAI_COMPLETIONS_MODEL!,
      name: "E2E model",
      reasoning: false,
      input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: 128000,
      maxTokens: 4096,
    }],
  });
}
