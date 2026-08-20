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
  certs/modeltap-ca-cert.pem \
  certs/modeltap-ca-key.pem
./target/debug/modeltap run config.test.yaml
```

The following is the local test configuration from `config.test.yaml`:

```yaml
proxy:
  listen: 127.0.0.1:8080

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
    provider: openai
    hosts:
      - chatgpt.com
    mitm: true
  - id: anthropic
    provider: anthropic
    hosts:
      - anthropic.com
    mitm: true
  - id: gemini
    provider: gemini
    hosts:
      - googleapis.com
    mitm: true
  - id: deepseek
    provider: deepseek
    hosts:
      - api.deepseek.com
    mitm: true
    egress: direct

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
`egress: direct` and never uses Privoxy.

Each `hosts` entry is a domain root. It matches the configured domain itself and
all of its subdomains at a DNS label boundary. For example,
`hosts: [googleapis.com]` matches both `googleapis.com` and
`generativelanguage.googleapis.com`, but not `notgoogleapis.com`. Do not assign
overlapping parent and child domains to different sites; configuration
validation rejects ambiguous domain trees.

The inbound proxy does not require client authentication, including when
`proxy.listen` uses a non-loopback address such as `0.0.0.0:8080`. A non-loopback
listener must be protected with a host firewall, private network, or another
trusted access-control layer to avoid operating an open proxy.

The library exposes provider-aware usage parsers for OpenAI
Chat/Responses/Embeddings, Anthropic, Gemini, and DeepSeek. Pricing uses decimal
arithmetic and daily peak/off-peak windows in the configured IANA timezone.

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
```

Body previews are capped at 4 KiB per chunk and authentication headers are never
logged. Debug output can still include prompt and model-response content; enable
it only in a trusted environment.

When `telemetry.otlp` is set, usage events are exported through OTLP/HTTP to
`<endpoint>/v1/metrics`. The exported metrics are `ai_proxy_requests`,
`ai_proxy_tokens`, and `ai_proxy_cost`; labels are limited to `site`,
`provider`, `model`, token type, price period, and currency.

For an end-to-end Grafana Cloud setup—including installing Grafana Alloy on
macOS or Linux, creating a least-privilege Cloud Access Policy token,
configuring remote write, validating the pipeline, and importing the bundled
dashboard—follow the [Grafana Alloy and Grafana Cloud guide](docs/index.html#alloy).
