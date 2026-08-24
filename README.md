# ModelTap

Know exactly how many tokens and how much money your AI coding agents use.

ModelTap is a transparent, network-level observability proxy for Codex, Claude
Code, Gemini CLI, oh-my-pi, and other AI clients. It measures the API traffic
that actually reaches model providers and exports token usage and estimated cost
to Grafana or any OTLP/HTTP-compatible backend.

**No SDK. No agent patches. No vendor-specific telemetry integration.**

![Model Grafana dashboard showing QPS, token usage, estimated cost, and agent breakdowns](https://tenfy.cn/picture/modeltap-grafana-dashboard.jpg)

ModelTap recognizes OpenAI, Anthropic, Gemini, and DeepSeek API usage; detects
common agent CLIs automatically; supports streaming SSE and WebSocket traffic;
and can forward directly or through HTTP, HTTPS, and SOCKS5 egress proxies.

## Why ModelTap?

Native agent telemetry and log parsers can be useful, but neither provides the
same provider-independent view of what left the machine. ModelTap observes the
model API boundary instead.

| | ModelTap | Native agent telemetry | Log-file parsers |
| --- | --- | --- | --- |
| One view across agent CLIs | Yes | Usually agent-specific | Varies |
| Measures API usage at the network boundary | Yes | Depends on the agent | No |
| Requires an agent plugin or code change | No | Often | No |
| Exports standard OTLP metrics | Yes | Varies | Varies |
| Works with a configured egress proxy | Yes | Depends on the agent | N/A |

Think of it as a purpose-built `mitmproxy` for AI model usage: it focuses on
tokens, estimated cost, agent identity, and OpenTelemetry rather than general
HTTP inspection.

## What you get

- Token totals and estimated cost, broken down by agent CLI, provider site, model, and token type.
- A bundled [Grafana dashboard](grafana/modeltap-dashboard.json) with filters for agent, site, and model.
- Standard OTLP/HTTP metrics for Grafana Alloy, Grafana Cloud, and compatible backends.
- Streaming-friendly inspection for HTTP/1.1, HTTP/2, SSE, and WebSocket traffic.

## Quick start

Install ModelTap and create the local root CA once. Keep the private key secret and install the generated root certificate in your operating system or client trust store:

### macOS / Linux

```shell
brew install tenfyzhong/tap/modeltap
mkdir -p certs
modeltap ca-init \
  --cert certs/modeltap-ca-cert.pem \
  --key certs/modeltap-ca-key.pem
cp config.sample.yaml config.yaml
modeltap validate --config config.yaml
modeltap run --config config.yaml
```

### Windows (PowerShell)

```powershell
# Install from source or download the prebuilt binary from Releases
cargo install --path .
New-Item -ItemType Directory -Force -Path certs
modeltap ca-init `
  --cert certs\modeltap-ca-cert.pem `
  --key certs\modeltap-ca-key.pem
Copy-Item config.sample.yaml config.yaml
modeltap validate --config config.yaml
modeltap run --config config.yaml
```

Before starting the proxy, open `config.yaml` and set the telemetry endpoint, egress route, and pricing rules for your environment. The bundled configuration uses a sample `privoxy` egress route; set `egress.default: direct` if you do not use an upstream proxy. `modeltap validate --config <CONFIG>` checks YAML, site/egress validation, and pricing rules without binding a listener or reading certificate files.

Shell completions are included in `completions/`. Load the appropriate file with `source completions/modeltap.bash`, `source completions/_modeltap`, `source completions/modeltap.fish`, or `. completions/modeltap.ps1` for Bash, Zsh, Fish, or PowerShell respectively.

### Installing and trusting the local root CA

To allow TLS interception without certificate verification errors:

- **macOS**: Import `certs/modeltap-ca-cert.pem` into the System keychain with Keychain Access or `security`:
  ```shell
  sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain certs/modeltap-ca-cert.pem
  ```

- **Linux**: Copy `certs/modeltap-ca-cert.pem` to `/usr/local/share/ca-certificates/modeltap-ca-cert.crt` and update CA certificates:
  ```shell
  sudo cp certs/modeltap-ca-cert.pem /usr/local/share/ca-certificates/modeltap-ca-cert.crt
  sudo update-ca-certificates
  ```

- **Windows**: Import `certs\modeltap-ca-cert.pem` into the Current User Trusted Root Certification Authorities store:
  ```powershell
  Import-Certificate -FilePath .\certs\modeltap-ca-cert.pem -CertStoreLocation Cert:\CurrentUser\Root
  ```
  Or using `certutil` from Command Prompt:
  ```cmd
  certutil -addstore -user Root certs\modeltap-ca-cert.pem
  ```
  Rust-based tools (like `codex`) and Windows system applications query the Windows Certificate Store automatically.

### Client configuration

After the proxy starts, configure your agent to use `http://127.0.0.1:2080`.

#### Node.js & Agent CLI clients (oh-my-pi, Claude Code)

Node.js uses its own bundled CA store and does not read system root stores by default. Point `NODE_EXTRA_CA_CERTS` at the ModelTap CA certificate (using an absolute path) before starting your agent:

**macOS / Linux (Bash / Zsh)**:
```shell
export NODE_EXTRA_CA_CERTS="$(pwd)/certs/modeltap-ca-cert.pem"
export HTTP_PROXY=http://127.0.0.1:2080
export HTTPS_PROXY=http://127.0.0.1:2080
export PI_PROXY=http://127.0.0.1:2080
omp
```

**macOS / Linux (Fish)**:
```shell
set -x NODE_EXTRA_CA_CERTS (pwd)/certs/modeltap-ca-cert.pem
set -x HTTP_PROXY http://127.0.0.1:2080
set -x HTTPS_PROXY http://127.0.0.1:2080
set -x PI_PROXY http://127.0.0.1:2080
omp
```

**Windows (PowerShell)**:
```powershell
$env:NODE_EXTRA_CA_CERTS = "$pwd\certs\modeltap-ca-cert.pem"
$env:HTTP_PROXY = "http://127.0.0.1:2080"
$env:HTTPS_PROXY = "http://127.0.0.1:2080"
$env:PI_PROXY = "http://127.0.0.1:2080"
omp
```

**Windows (Command Prompt)**:
```cmd
set NODE_EXTRA_CA_CERTS=%cd%\certs\modeltap-ca-cert.pem
set HTTP_PROXY=http://127.0.0.1:2080
set HTTPS_PROXY=http://127.0.0.1:2080
set PI_PROXY=http://127.0.0.1:2080
omp
```

`PI_PROXY` routes all oh-my-pi providers through ModelTap. Provider-specific variables override it. For example, use `PI_PROXY_CURSOR` when Cursor needs a different proxy endpoint:

```shell
export PI_PROXY_CURSOR=http://127.0.0.1:2080
```

oh-my-pi uses a dedicated HTTP/2 transport for Cursor Agent requests; setting `PI_PROXY` (or its `PI_PROXY_CURSOR` override) ensures Cursor models, including Grok, reach ModelTap. Without this setting, MITM requests can fail certificate verification and some clients may misleadingly report that their Google API key or OAuth credential is missing.

#### Python & other CLI tools

- **Python (Requests / HTTPX / OpenAI SDK / Anthropic SDK)**:
  - macOS / Linux (Bash): `export REQUESTS_CA_BUNDLE="$(pwd)/certs/modeltap-ca-cert.pem" SSL_CERT_FILE="$(pwd)/certs/modeltap-ca-cert.pem"`
  - Windows (PowerShell): `$env:REQUESTS_CA_BUNDLE = "$pwd\certs\modeltap-ca-cert.pem"; $env:SSL_CERT_FILE = "$pwd\certs\modeltap-ca-cert.pem"`
- **Git**:
  - macOS / Linux (Bash): `git config --global http.sslCAInfo "$(pwd)/certs/modeltap-ca-cert.pem"`
  - Windows (PowerShell): `git config --global http.sslCAInfo "$pwd\certs\modeltap-ca-cert.pem"`

### Running as a background service

To run ModelTap persistently in the background on startup, configure it as an operating system service:

#### macOS & Linux (Homebrew services)

If installed via Homebrew, manage ModelTap directly with `brew services`:

```shell
# Start ModelTap service in background
brew services start tenfyzhong/tap/modeltap

# Check service status
brew services info tenfyzhong/tap/modeltap

# Restart or stop the service
brew services restart tenfyzhong/tap/modeltap
brew services stop tenfyzhong/tap/modeltap
```

#### Linux (systemd)

For standalone Linux installations without Homebrew, create `/etc/systemd/system/modeltap.service`:

```ini
[Unit]
Description=ModelTap AI Traffic Monitor
After=network.target

[Service]
Type=simple
User=modeltap
WorkingDirectory=/opt/modeltap
ExecStart=/opt/modeltap/modeltap run --config /opt/modeltap/config.yaml
Restart=always
RestartSec=5
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

Enable and start the service:

```shell
sudo systemctl daemon-reload
sudo systemctl enable --now modeltap
```

#### Windows Service (NSSM)

[NSSM (Non-Sucking Service Manager)](https://nssm.cc/) is the simplest way to run ModelTap as a Windows service with automatic restart on crash:

```powershell
# 1. Install NSSM (via Scoop or Winget)
winget install NSSM.NSSM
# or: scoop install nssm

# 2. Install and configure ModelTap as a service (Run PowerShell as Administrator)
nssm install ModelTap "C:\modeltap\modeltap.exe" "run --config C:\modeltap\config.yaml"
nssm set ModelTap AppDirectory "C:\modeltap"
nssm set ModelTap Start SERVICE_AUTO_START
nssm set ModelTap AppStdout "C:\modeltap\logs\service-stdout.log"
nssm set ModelTap AppStderr "C:\modeltap\logs\service-stderr.log"

# 3. Start the service
nssm start ModelTap

# 4. Service management
nssm status ModelTap
nssm restart ModelTap
nssm stop ModelTap
```

#### Windows Service (WinSW)

Alternatively, download [WinSW](https://github.com/winsw/winsw/releases) (e.g. `WinSW-x64.exe`), place it as `modeltap-service.exe` next to `modeltap.exe`, and create `modeltap-service.xml`:

```xml
<service>
  <id>ModelTap</id>
  <name>ModelTap AI Proxy</name>
  <description>ModelTap AI traffic monitor and cost proxy</description>
  <executable>C:\modeltap\modeltap.exe</executable>
  <arguments>run --config C:\modeltap\config.yaml</arguments>
  <workingdirectory>C:\modeltap</workingdirectory>
  <logpath>C:\modeltap\logs</logpath>
  <log mode="roll-by-size">
    <sizeThreshold>10240</sizeThreshold>
    <keepFiles>5</keepFiles>
  </log>
</service>
```

Install and start the service:

```powershell
.\modeltap-service.exe install
.\modeltap-service.exe start
```

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

Use a single `rates` block for fixed pricing, or configure `peak_windows` with separate `peak` and `off_peak` rates. Peak windows can be configured globally under `pricing.peak_windows` or customized per model/rule:

```yaml
pricing:
  timezone: Asia/Shanghai
  peak_windows:
    - weekdays: [1, 2, 3, 4, 5]
      start: "09:00"
      end: "12:00"
    - weekdays: [1, 2, 3, 4, 5]
      start: "14:00"
      end: "18:00"
  rules:
    # Uses global peak windows (Mon-Fri 09:00-12:00, 14:00-18:00)
    - model: deepseek-*
      currency: USD
      peak:
        input: 0.445
        output: 1.336
      off_peak:
        input: 0.223
        output: 0.668
    # Custom peak windows overriding global defaults for a specific model
    - model: custom-night-*
      currency: USD
      peak_windows:
        - weekdays: [5, 6]
          start: "22:00"
          end: "02:00"
      peak:
        input: 2.0
        output: 4.0
      off_peak:
        input: 1.0
        output: 2.0
    # Optional exact rates for server-confirmed Codex Fast responses.
    # When omitted, Fast responses use twice the regular rate for compatibility.
    - model: gpt-5-codex
      currency: USD
      rates:
        input: 1.5
        output: 6.0
      fast:
        input: 3.0
        output: 12.0
```

Each peak window specifies `start` and `end` times in `HH:MM` format and an optional `weekdays` array (e.g. `weekdays: [1, 2, 3, 4, 5]` or `weekdays: ["Mon", "Tue", "Wed", "Thu", "Fri"]`, where `1` / `"Mon"` / `"Monday"` = Monday through `7` / `"Sun"` / `"Sunday"` = Sunday; defaults to all days `[1, 2, 3, 4, 5, 6, 7]`). Windows may cross midnight (for example `start: "22:00"`, `end: "02:00"`) but must not overlap on the same day.

## Telemetry and observability

When `telemetry.otlp` is set, usage events are exported through OTLP/HTTP to
`<endpoint>/v1/metrics`. The exported metrics are `ai_proxy_requests`,
`ai_proxy_tokens`, and `ai_proxy_cost`; labels are limited to `site`,
`model`, `agent_cli`, token type, price period, currency, and billing mode.
For Codex responses that the server reports as Fast mode (`service_tier: priority`),
ModelTap uses the matching rule's optional `fast` price block and exports their
cost with `billing_mode="fast"`. If no `fast` price is configured, it applies a
2x multiplier to the regular price for backward compatibility. Requests that merely
ask for Fast mode, but are not served in it, retain default billing.

For monitored HTTP requests, ModelTap requests an uncompressed upstream response
(`Accept-Encoding: identity`) so that usage data can be parsed accurately.

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

## Frequently Asked Questions (FAQ)

### Codex fails with `invalid peer certificate: BadSignature`

**Symptom**: Running `codex` (or other Rust-based CLI tools) through ModelTap outputs:

```text
Falling back from WebSockets to HTTPS transport.
stream disconnected before completion: invalid peer certificate: BadSignature
```

**Cause**: A stale `modeltap local CA` root certificate exists in your system or login keychain with a different public/private key pair than the active CA private key configured in ModelTap. `codex` loads the old root certificate from Keychain, and TLS verification fails because the signature on the dynamically generated leaf certificate was produced by the new private key.

**Resolution**:

1. Remove the old certificate from the macOS Keychain:

   ```shell
   security delete-certificate -c "modeltap local CA" ~/Library/Keychains/login.keychain-db 2>/dev/null || true
   sudo security delete-certificate -c "modeltap local CA" /Library/Keychains/System.keychain 2>/dev/null || true
   ```

2. Re-install and trust the active ModelTap root CA certificate:

   ```shell
   sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain <path-to-ca-cert.pem>
   ```

3. Verify that the serial number and public key match between the Keychain and your CA certificate file:

   ```shell
   security find-certificate -c "modeltap local CA" -p | openssl x509 -noout -serial -pubkey
   openssl x509 -in <path-to-ca-cert.pem> -noout -serial -pubkey
   ```

### Codex fails with `invalid peer certificate: UnknownIssuer`

**Symptom**: Running `codex` outputs:

```text
Falling back from WebSockets to HTTPS transport.
stream disconnected before completion: invalid peer certificate: UnknownIssuer
```

**Cause**: The ModelTap CA certificate is present in the keychain file, but lacks root trust policy settings (for example, `security add-trusted-cert -d` was executed without `sudo`, preventing macOS from writing the admin trust settings). Tools using `rustls-native-certs` only load root certificates configured with explicit trust settings.

**Resolution**:

- **System-wide trust (recommended)**: Run with `sudo` to write to the admin trust settings:

  ```shell
  sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain <path-to-ca-cert.pem>
  ```

- **User-level trust**: Run without `-d` (and without `sudo`), which triggers a macOS password / Touch ID prompt to authorize user trust:

  ```shell
  security add-trusted-cert -r trustRoot -k ~/Library/Keychains/login.keychain-db <path-to-ca-cert.pem>
  ```

- **Verify trust settings**: Confirm that `modeltap local CA` appears in trust settings:

  ```shell
  security dump-trust-settings -d | grep -A 2 "modeltap local CA" || security dump-trust-settings | grep -A 2 "modeltap local CA"
  ```

## Contributing and architecture

For architecture details, internal protocol parser design, agent CLI detection rules, and testing workflows, see [CONTRIBUTING.md](CONTRIBUTING.md).
