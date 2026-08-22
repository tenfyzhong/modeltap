# ModelTap

ModelTap monitors AI model usage, token consumption, and estimated cost, then
exports those measurements through OTLP/HTTP to observability systems such as
Grafana Alloy and Grafana Cloud. It runs as a Rust explicit proxy, recognizes
usage reported by OpenAI, Anthropic, Gemini, and DeepSeek APIs, and applies
configurable model pricing to the collected usage.

The proxy supports direct forwarding or a cascading HTTP/HTTPS/SOCKS5 egress
proxy (including GOST), TLS MITM for configured hosts, HTTP/1.1 and HTTP/2
upstream connections, and streaming SSE pass-through without buffering full
responses.

For MITM hosts that use WebSockets, including Codex's `chatgpt.com` endpoint, the
proxy forwards the HTTP/1.1 upgrade and then tunnels WebSocket frames
bidirectionally without buffering them. It side-parses server text frames for
OpenAI Responses `response.completed` usage events and exports their tokens and
costs through the normal telemetry pipeline. Negotiated `permessage-deflate`
frames are decompressed only in this observation path, including fragmented
messages and context takeover; the original handshake and frames are forwarded
unchanged.

Build the binary, generate the local root CA once, install the certificate in
each client trust store, and protect the private key as a secret:

```shell
make build
mkdir -p certs
./target/debug/modeltap ca-init \
  --cert certs/modeltap-ca-cert.pem \
  --key certs/modeltap-ca-key.pem
cp config.sample.yaml config.yaml
./target/debug/modeltap run --config config.yaml
./target/debug/modeltap validate --config config.yaml
```

Use `modeltap validate --config <CONFIG>` to check YAML syntax, site/egress validation,
and pricing rules without binding a listener or reading certificate files.

Shell completions are included in `completions/`. Load the appropriate file
with `source completions/modeltap.bash`, `source completions/_modeltap`, or
`source completions/modeltap.fish` for Bash, Zsh, or Fish respectively.

### Node.js clients

Node.js uses its own CA bundle and may not trust a locally installed ModelTap
root CA. Before starting a Node.js client such as oh-my-pi, point
`NODE_EXTRA_CA_CERTS` at the ModelTap CA certificate. Use an absolute path and
restart the client (including any background daemon) after setting it:

```shell
export NODE_EXTRA_CA_CERTS="$(pwd)/certs/modeltap-ca-cert.pem"
export HTTP_PROXY=http://127.0.0.1:2080
export HTTPS_PROXY=http://127.0.0.1:2080
export PI_PROXY=http://127.0.0.1:2080
omp
```

`PI_PROXY` routes all oh-my-pi providers through ModelTap. Provider-specific
variables override it. For example, use `PI_PROXY_CURSOR` when Cursor needs a
different proxy endpoint:

```shell
export PI_PROXY_CURSOR=http://127.0.0.1:2080
```

oh-my-pi uses a dedicated HTTP/2 transport for Cursor Agent requests; setting
`PI_PROXY` (or its `PI_PROXY_CURSOR` override) ensures Cursor models, including
Grok, reach ModelTap. For fish, use `set -x PI_PROXY http://127.0.0.1:2080`
(and set `NODE_EXTRA_CA_CERTS` similarly).
Without this setting, MITM requests can fail certificate verification and some
clients may misleadingly report that their Google API key or OAuth credential is
missing.

Copy [`config.sample.yaml`](config.sample.yaml) before first use, then adjust its
certificate paths, telemetry endpoint, egress proxy, sites, and pricing rules.
The sample includes a complete local configuration, including egress, sites,
and pricing rules. The following shows the equivalent local configuration:

```yaml
proxy:
  listen: 127.0.0.1:2080

logging:
  level: info

tls:
  ca_cert_file: ./certs/modeltap-ca-cert.pem
  ca_key_file: ./certs/modeltap-ca-key.pem

telemetry:
  otlp:
    endpoint: http://127.0.0.1:4318
    service_name: modeltap-test

egress:
  default: privoxy
  proxies:
    - id: privoxy
      url: http://127.0.0.1:8118

sites:
  - id: openai
    hosts:
      - chatgpt.com
  - id: anthropic
    hosts:
      - anthropic.com
  - id: gemini
    hosts:
      - googleapis.com
  - id: deepseek
    hosts:
      - api.deepseek.com
    egress: direct
  - id: grok
    hosts:
      - api.x.ai
  - id: cursor
    hosts:
      - api2.cursor.sh

pricing:
  timezone: Asia/Shanghai
  # Official first-party API list prices checked on 2026-08-20.
  # All rates are USD per 1M tokens. DeepSeek CNY prices were converted with
  # the 2026-08-19 ECB rates (1 CNY = 0.148407227899 USD).
  peak_windows:
    - start: "09:00"
      end: "12:00"
    - start: "14:00"
      end: "18:00"
  rules:
    - site: openai
      model: "gpt-5.6-sol*"
      currency: USD
      rates:
        input: 5
        output: 30
        cache_read: 0.5
    - site: openai
      model: "gpt-5.6-terra*"
      currency: USD
      rates:
        input: 2
        output: 12
        cache_read: 0.2
    - site: openai
      model: "gpt-5.6-luna*"
      currency: USD
      rates:
        input: 0.2
        output: 1.2
        cache_read: 0.02
    - site: anthropic
      model: "claude-opus-4-8*"
      currency: USD
      rates:
        input: 5
        output: 25
        cache_read: 0.5
        cache_write: 6.25
    - site: anthropic
      model: "claude-sonnet-4-6*"
      currency: USD
      rates:
        input: 3
        output: 15
        cache_read: 0.3
        cache_write: 3.75
    - site: anthropic
      model: "claude-haiku-4-5*"
      currency: USD
      rates:
        input: 1
        output: 5
        cache_read: 0.1
        cache_write: 1.25
    - site: gemini
      model: "gemini-3.7-flash*"
      currency: USD
      rates:
        input: 0.75
        output: 3.75
        cache_read: 0.075
    - site: deepseek
      model: "deepseek-v4-flash*"
      currency: USD
      peak:
        input: 0.445221684
        output: 1.335665051
        cache_read: 0.014840723
      off_peak:
        input: 0.222610842
        output: 0.667832526
        cache_read: 0.007420361
    - site: deepseek
      model: "deepseek-v4-pro*"
      currency: USD
      peak:
        input: 1.335665051
        output: 4.006995153
        cache_read: 0.044522168
      off_peak:
        input: 0.667832526
        output: 2.003497577
        cache_read: 0.022261084
```

With this configuration, OpenAI, Anthropic, and Gemini use the default Privoxy
egress at `127.0.0.1:8118`. DeepSeek overrides the default with
`egress: direct` and never uses Privoxy. Grok uses the OpenAI-compatible API at
`api.x.ai`; Cursor uses `api2.cursor.sh`.

### Sites and protocol detection

`id` is the site identity used in metric labels and `pricing.rules`; use the
actual service/vendor name, such as `grok`, `cursor`, or `openai`. ModelTap
detects the usage protocol from each request or response automatically, so site
configuration does not need a `provider` or `provider_type` field. It recognizes
Cursor Connect/Protobuf, Gemini usage metadata, Anthropic message events, and
OpenAI Chat/Responses payloads. A DeepSeek site can therefore report both its
OpenAI-compatible and Anthropic-compatible traffic without any special setting.

Every host configured under `sites` is always intercepted with TLS MITM. This
makes `sites` the explicit allowlist of traffic that ModelTap can inspect. Hosts
absent from `sites` are forwarded without MITM and no usage is collected. Remove
the former `provider`, `provider_type`, and `mitm` fields when migrating an
existing configuration.

Each `hosts` entry is a domain root. It matches the configured domain itself and
all of its subdomains at a DNS label boundary. For example,
`hosts: [googleapis.com]` matches both `googleapis.com` and
`generativelanguage.googleapis.com`, but not `notgoogleapis.com`. Do not assign
overlapping parent and child domains to different sites; configuration
validation rejects ambiguous domain trees.

The inbound proxy does not require client authentication, including when
`proxy.listen` uses a non-loopback address such as `0.0.0.0:2080`. A non-loopback
listener must be protected with a host firewall, private network, or another
trusted access-control layer to avoid operating an open proxy.

The library automatically detects usage protocols for OpenAI
Chat/Responses/Embeddings, Anthropic, Gemini, DeepSeek, and Cursor Agent.
Cursor Agent traffic uses Connect/Protobuf: ModelTap reads the selected model ID
from each request, so Cursor models such as GPT, Claude, Grok, GLM, Gemini, and
Composer are reported without a model allowlist. Cursor reports generated-token
increments; configure `pricing.rules` for `site: cursor` when you want costs.
Pricing uses decimal arithmetic and daily peak/off-peak windows in the configured IANA timezone.
DeepSeek accepts both its native OpenAI-compatible responses and the Anthropic
compatible streaming responses used by Claude Code. Usage metrics include an
`agent_cli` attribute inferred from client headers (`claude_code`, `codex`,
`gemini_cli`, `oh_my_pi`, `opencode`, `pi`, `github_copilot`, `amazon_q`,
`roo_code`, `qwen_code`, `factory_droid`, `crush`, `kiro`, `qoder`,
`antigravity`, `cursor`, or `unknown`). ModelTap ships stable built-in rules
rather than exporting raw User-Agent values, which would create high-cardinality
metrics. Tools without a distinctive request header, including Aider, Goose, and
Continue, remain `unknown` rather than risking an incorrect classification.

### Agent CLI E2E workflow

[`Agent CLI E2E`](.github/workflows/agent-e2e.yml) is a manually triggered
GitHub Actions workflow. It installs the agent CLIs, routes each request through
ModelTap, and checks the exported Prometheus metrics for a positive request and
token total with the expected `agent_cli` label. It does not run on pull
requests, because it makes billable API requests.

The workflow additionally routes representative requests for every documented
`agent_cli` value through a local HTTPS upstream. This keeps proprietary,
OAuth-only, and IDE-only agents covered by the same metric assertion without
claiming that their vendor client ran in CI.

The workflow uses a local protocol-compatible upstream and a generated test CA;
it requires no model credentials, external base URL, or billable API calls.
The following configuration names are retained as examples for running the same
clients against a real provider outside CI:

| Protocol and client | Secrets | Repository variable |
| --- | --- | --- |
| OpenAI Chat Completions via OpenCode | `OPENAI_COMPLETIONS_API_KEY`, `OPENAI_COMPLETIONS_BASE_URL` | `OPENAI_COMPLETIONS_MODEL` |
| OpenAI Responses via Codex | `OPENAI_RESPONSES_API_KEY`, `OPENAI_RESPONSES_BASE_URL` | `OPENAI_RESPONSES_MODEL` |
| Anthropic Messages via Claude Code | `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL` | `ANTHROPIC_MODEL` |
| Gemini API via Gemini CLI | `GEMINI_API_KEY`, `GEMINI_BASE_URL` | `GEMINI_MODEL` |

Base URLs must include any API prefix required by the provider, such as `/v1`.
The workflow derives its TLS interception hosts from these URLs at runtime, so
do not use a URL that redirects to another API hostname.

The table distinguishes real E2E workflow coverage from simulated protocol and
User-Agent regression coverage. A simulated check validates ModelTap's request
classification and supported response protocol parsing, but does not claim that
the vendor client was installed or authenticated in CI.

| Agent | `agent_cli` | Detection | Verification |
| --- | --- | --- | --- |
| Claude Code | `claude_code` | `claude-code/` or `claude-cli/` User-Agent | Real E2E workflow |
| Codex | `codex` | `codex` User-Agent | Real E2E workflow |
| oh-my-pi | `oh_my_pi` | `oh-my-pi` User-Agent or Cursor request headers | Real E2E workflow |
| Gemini CLI | `gemini_cli` | `GeminiCLI` User-Agent | Real E2E workflow |
| OpenCode | `opencode` | `opencode` User-Agent | Real E2E workflow |
| Pi | `pi` | `pi (` User-Agent | Real E2E workflow |
| GitHub Copilot CLI | `github_copilot` | `copilot/` User-Agent | Real E2E workflow |
| Amazon Q | `amazon_q` | `AmazonQ-For-CLI` User-Agent | Simulated protocol + User-Agent regression |
| Roo Code | `roo_code` | `RooCode/` User-Agent | Simulated protocol + User-Agent regression |
| Qwen Code | `qwen_code` | `QwenCode/` User-Agent | Real E2E workflow |
| Factory Droid | `factory_droid` | `factory-cli/` User-Agent | Simulated protocol + User-Agent regression |
| Crush | `crush` | `Charm-Crush/` User-Agent | Simulated protocol + User-Agent regression |
| Kiro | `kiro` | `kiro-ide/` User-Agent | Simulated protocol + User-Agent regression |
| Qoder | `qoder` | `Qoder-Cli` User-Agent | Simulated protocol + User-Agent regression |
| Antigravity | `antigravity` | `antigravity/` User-Agent | Simulated protocol + User-Agent regression |
| Cursor Agent | `cursor` | Cursor Connect/Protobuf request | Simulated protocol + User-Agent regression |

Use a single `rates` block for a model whose prices do not vary by time. These
rules can coexist with global `peak_windows` used by other models:

```yaml
pricing:
  timezone: Asia/Shanghai
  rules:
    - site: openai
      model: text-embedding-3-*
      currency: USD
      rates:
        input: 0.02
        output: 0
```

Each peak window has separate `start` and `end` fields in `HH:MM` format. Windows
may cross midnight (for example `start: "22:00"`, `end: "02:00"`) but must not
overlap. The legacy string form (`"09:00-12:00"`) remains accepted for existing
configurations.

## Debug logging

Set `logging.level: debug` to log CONNECT targets, request routing, response
status, SSE detection, parsed usage, and a preview of each forwarded request and
response chunk:

```yaml
logging:
  level: debug
  # file: ./logs/modeltap.log
```

Body previews are capped at 4 KiB per chunk and authentication headers are never
logged. Set `logging.file` to append the same logs to a file while retaining
stderr output; ModelTap creates the file but not its parent directory. Debug
output can still include prompt and model-response content; enable it only in a
trusted environment.

When `telemetry.otlp` is set, usage events are exported through OTLP/HTTP to
`<endpoint>/v1/metrics`. The exported metrics are `ai_proxy_requests`,
`ai_proxy_tokens`, and `ai_proxy_cost`; labels are limited to `site`,
`model`, `agent_cli`, token type, price period, and currency.

For an end-to-end Grafana Cloud setup—including installing Grafana Alloy on
macOS or Linux, creating a least-privilege Cloud Access Policy token,
configuring remote write, validating the pipeline, and importing the bundled
dashboard—follow the [Grafana Alloy and Grafana Cloud guide](docs/index.html#alloy).
