# Contributing to ModelTap

Thank you for contributing. This project is a security-sensitive proxy, so changes must preserve transparent forwarding, safe certificate handling, and low telemetry cardinality.

## Development setup

Install Rust 1.85 or newer, then run:

```bash
make build
make test
make fmt
make check
```

Use a feature or fix branch. Do not commit generated certificates, private keys, Docker credentials, or Grafana Cloud credentials.

## Development rules

- Add or update a reusable test before changing behavior.
- Keep all new documentation and source comments in English.
- Run `cargo fmt`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` before opening a pull request.
- Keep MITM parsing observational: do not buffer complete SSE streams or alter upstream payloads.
- Treat certificate private keys and proxy credentials as secrets. Load credentials from environment variables, mounted files, or a secret manager.
- Avoid unbounded Prometheus or OTLP labels. Request IDs, user IDs, API keys, and full URLs must not become metric labels.

## Architecture and implementation details

### Streaming proxy and WebSocket tunneling

- **Transparent MITM**: ModelTap performs TLS interception only for hosts specified under `sites` and passes all non-configured traffic through unmodified without inspection.
- **Streaming SSE pass-through**: Server-Sent Events (SSE) streams are parsed incrementally on the fly without buffering full responses, ensuring minimal memory usage and zero added latency.
- **WebSocket inspection**: For MITM hosts that use WebSockets (such as Codex's `chatgpt.com` endpoint), the proxy forwards the HTTP/1.1 upgrade and tunnels WebSocket frames bidirectionally without buffering. It side-parses server text frames for OpenAI Responses `response.completed` usage events and exports their tokens and costs through the normal telemetry pipeline. Negotiated `permessage-deflate` frames are decompressed only in this observation path (supporting fragmented messages and context takeover), while the original handshake and wire frames are forwarded unchanged.

### Sites and protocol detection

- **Automatic protocol detection**: ModelTap dynamically recognizes usage protocols from request paths, headers, and response formats without requiring a static `provider` or `provider_type` configuration. Supported protocols include:
  - OpenAI Chat Completions, Responses API, and Embeddings
  - Anthropic Messages API (including streaming SSE event streams)
  - Gemini API (including `usageMetadata` chunks)
  - DeepSeek (both native OpenAI-compatible responses and Anthropic-compatible streaming used by Claude Code)
  - Cursor Connect / Protobuf streams (reading selected model IDs and generated-token increments)
- **Domain root matching**: Each `hosts` entry acts as a domain root, matching the domain itself and all subdomains at DNS label boundaries (e.g., `googleapis.com` matches `generativelanguage.googleapis.com` but not `notgoogleapis.com`). Ambiguous overlapping parent and child domain trees across different sites are rejected by configuration validation.
- **Security model**: The proxy listener does not require client authentication. When listening on a non-loopback address (e.g., `0.0.0.0:2080`), access must be restricted via host firewalls, private networks, or another trusted access-control layer.

### Agent CLI classification

ModelTap infers the `agent_cli` metric label from client request headers and User-Agent patterns rather than emitting raw User-Agent strings, keeping metric cardinality bounded:

| Agent | `agent_cli` | Detection rule |
| --- | --- | --- |
| Claude Code | `claude_code` | `claude-code/` or `claude-cli/` User-Agent, `x-claude-code-session-id` header |
| Codex | `codex` | `codex` User-Agent, `originator: codex_exec` header |
| oh-my-pi | `oh_my_pi` | `oh-my-pi`/`omp` User-Agent, `x-oh-my-pi`/`x-omp`, `x-ghost-mode`, or Cursor CLI header |
| Gemini CLI | `gemini_cli` | `GeminiCLI` User-Agent, `x-gemini-api-privileged-user-id` header |
| OpenCode | `opencode` | `opencode` User-Agent, `originator: opencode` header |
| Pi | `pi` | `pi` User-Agent, `x-opencode-client: pi`, `X-OpenRouter-Title: pi`, `X-BILLING-INVOKE-ORIGIN: Pi` |
| GitHub Copilot CLI | `github_copilot` | `copilot/` User-Agent, `x-interaction-type` header |
| Amazon Q | `amazon_q` | `AmazonQ-For-CLI` User-Agent |
| Roo Code | `roo_code` | `RooCode/` User-Agent |
| Qwen Code | `qwen_code` | `QwenCode/` User-Agent |
| Factory Droid | `factory_droid` | `factory-cli/` User-Agent |
| Crush | `crush` | `Charm-Crush/` User-Agent |
| Kiro | `kiro` | `kiro-ide/` User-Agent |
| Qoder | `qoder` | `Qoder-Cli` User-Agent |
| Antigravity | `antigravity` | `antigravity/` User-Agent |
| Cursor Agent | `cursor` | Cursor Connect/Protobuf request without oh-my-pi headers |

Tools without distinctive headers (such as Aider, Goose, and Continue) remain classified as `unknown` to avoid misattribution.

## Testing and CI workflows

### Agent CLI E2E workflow

[`Agent CLI E2E`](.github/workflows/agent-e2e.yml) runs on pull requests and main pushes. It installs agent CLIs, routes their requests through ModelTap, and asserts that Prometheus/OTLP metrics record positive request and token counts with the expected `agent_cli` label.

- **Zero-credential testing**: CI uses a local protocol-compatible mock upstream and generated test CA, requiring no external API keys or billable requests.
- **Simulated vs. Real coverage**: The test suite covers both real installed client runs and simulated protocol/header regression suites for proprietary/OAuth agents:

| Agent | `agent_cli` | Verification type |
| --- | --- | --- |
| Claude Code | `claude_code` | Real E2E workflow |
| Codex | `codex` | Real E2E workflow |
| oh-my-pi | `oh_my_pi` | Real E2E workflow |
| Gemini CLI | `gemini_cli` | Real E2E workflow |
| OpenCode | `opencode` | Real E2E workflow |
| Pi | `pi` | Real E2E workflow |
| GitHub Copilot CLI | `github_copilot` | Real E2E workflow |
| Amazon Q | `amazon_q` | Simulated protocol + User-Agent regression |
| Roo Code | `roo_code` | Simulated protocol + User-Agent regression |
| Qwen Code | `qwen_code` | Real E2E workflow |
| Factory Droid | `factory_droid` | Simulated protocol + User-Agent regression |
| Crush | `crush` | Simulated protocol + User-Agent regression |
| Kiro | `kiro` | Simulated protocol + User-Agent regression |
| Qoder | `qoder` | Simulated protocol + User-Agent regression |
| Antigravity | `antigravity` | Simulated protocol + User-Agent regression |
| Cursor Agent | `cursor` | Simulated protocol + User-Agent regression |

### Manual testing with real providers

When testing against real upstream providers outside CI, the workflow supports the following configuration variables:

| Protocol and client | Secrets | Repository variable |
| --- | --- | --- |
| OpenAI Chat Completions via OpenCode | `OPENAI_COMPLETIONS_API_KEY`, `OPENAI_COMPLETIONS_BASE_URL` | `OPENAI_COMPLETIONS_MODEL` |
| OpenAI Responses via Codex | `OPENAI_RESPONSES_API_KEY`, `OPENAI_RESPONSES_BASE_URL` | `OPENAI_RESPONSES_MODEL` |
| Anthropic Messages via Claude Code | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` | `ANTHROPIC_MODEL` |
| Gemini API via Gemini CLI | `GEMINI_API_KEY`, `GEMINI_BASE_URL` | `GEMINI_MODEL` |

## Pull requests

Describe the user-visible behavior, configuration changes, security implications, and test coverage. If a change affects the public configuration schema, update `README.md`, `docs/index.html`, and the example configuration files in the same pull request.

## Reporting security issues

Do not open a public issue for a vulnerability involving TLS interception, credentials, request content, or telemetry data exposure. Contact the repository maintainers privately through the security contact configured by the repository owner.
