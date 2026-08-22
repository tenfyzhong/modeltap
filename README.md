# ModelTap

ModelTap monitors AI model usage, token consumption, and estimated cost, then
exports those measurements through OTLP/HTTP to observability systems such as
Grafana Alloy and Grafana Cloud. It runs as a Rust explicit proxy, recognizes
usage reported by OpenAI, Anthropic, Gemini, and DeepSeek APIs, and applies
configurable model pricing to the collected usage.

The proxy supports direct forwarding or a cascading HTTP/HTTPS/SOCKS5 egress
proxy (including GOST), TLS MITM for configured hosts, HTTP/1.1 and HTTP/2
upstream connections, streaming SSE pass-through without buffering full
responses, and bidirectional WebSocket tunneling.

## Quick start

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

### Client configuration

#### Node.js & Agent CLI clients

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

## Configuration

Copy [`config.sample.yaml`](config.sample.yaml) to `config.yaml` before first use,
then adjust its certificate paths, telemetry endpoint, egress proxy, sites, and
pricing rules. See [`config.sample.yaml`](config.sample.yaml) for a complete
sample configuration.

### Sites and egress routing

- **Hosts allowlist**: Every host configured under `sites` is intercepted with TLS MITM. Each entry matches the domain root and all subdomains at DNS label boundaries (e.g., `googleapis.com` matches `generativelanguage.googleapis.com`). Non-configured hosts are forwarded transparently without MITM.
- **Egress routing**: The proxy supports default and per-site egress routing to upstream HTTP/HTTPS/SOCKS5 proxies or direct connections.
- **Automatic detection**: ModelTap automatically detects API protocols (OpenAI, Anthropic, Gemini, DeepSeek, Cursor) and client agent identities (`claude_code`, `codex`, `oh_my_pi`, `gemini_cli`, etc.) without requiring manual provider flags.

### Pricing rules

Pricing rules can be configured globally (without a `site`) or with site-specific overrides. When calculating costs, ModelTap first checks for a site-specific rule matching the request's site; if none matches, it falls back to matching global rules.

Use a single `rates` block for fixed pricing, or configure `peak_windows` with separate `peak` and `off_peak` rates:

```yaml
pricing:
  timezone: Asia/Shanghai
  rules:
    # Global rule applicable to any site
    - model: text-embedding-3-*
      currency: USD
      rates:
        input: 0.02
        output: 0
    # Site-specific override
    - site: custom_gateway
      model: text-embedding-3-*
      currency: USD
      rates:
        input: 0.015
        output: 0
```

Each peak window has separate `start` and `end` fields in `HH:MM` format. Windows may cross midnight (for example `start: "22:00"`, `end: "02:00"`) but must not overlap.

## Telemetry and observability

When `telemetry.otlp` is set, usage events are exported through OTLP/HTTP to
`<endpoint>/v1/metrics`. The exported metrics are `ai_proxy_requests`,
`ai_proxy_tokens`, and `ai_proxy_cost`; labels are limited to `site`,
`model`, `agent_cli`, token type, price period, and currency.

For an end-to-end Grafana Cloud setup—including installing Grafana Alloy on
macOS or Linux, creating a least-privilege Cloud Access Policy token,
configuring remote write, validating the pipeline, and importing the bundled
dashboard—follow the [Grafana Alloy and Grafana Cloud guide](https://tenfy.cn/modeltap/#alloy).

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
stderr output. Debug output can still include prompt and model-response content;
enable it only in a trusted environment.

## Contributing and architecture

For architecture details, internal protocol parser design, agent CLI detection rules, and testing workflows, see [CONTRIBUTING.md](CONTRIBUTING.md).
