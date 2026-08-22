#!/usr/bin/env bash
set -euo pipefail
set +x

readonly ARTIFACTS_DIR="$GITHUB_WORKSPACE/.github/e2e/artifacts"
readonly CONFIG_PATH="$RUNNER_TEMP/modeltap-e2e.yaml"
readonly CERT_DIR="$RUNNER_TEMP/modeltap-e2e-certs"
readonly CA_CERT="$CERT_DIR/modeltap-ca-cert.pem"
readonly SIMULATED_HOST="modeltap-e2e-simulated.local"
readonly SIMULATED_PORT=8443

export ARTIFACTS_DIR CERT_DIR CA_CERT SIMULATED_HOST

mkdir -p "$ARTIFACTS_DIR" "$CERT_DIR"

require_environment() {
  local name
  for name in "$@"; do
    if [[ -z "${!name:-}" ]]; then
      echo "Required GitHub secret or variable $name is not configured." >&2
      exit 1
    fi
  done
}

./target/debug/modeltap ca-init \
  --cert "$CA_CERT" \
  --key "$CERT_DIR/modeltap-ca-key.pem"

sudo cp "$CA_CERT" /usr/local/share/ca-certificates/modeltap-e2e.crt
sudo update-ca-certificates
echo "127.0.0.1 $SIMULATED_HOST" | sudo tee -a /etc/hosts >/dev/null

openssl req -new -newkey rsa:2048 -nodes \
  -subj "/CN=$SIMULATED_HOST" \
  -addext "subjectAltName=DNS:$SIMULATED_HOST" \
  -keyout "$CERT_DIR/simulated-upstream-key.pem" \
  -out "$CERT_DIR/simulated-upstream.csr" >/dev/null 2>&1
openssl x509 -req -days 1 \
  -in "$CERT_DIR/simulated-upstream.csr" \
  -CA "$CA_CERT" -CAkey "$CERT_DIR/modeltap-ca-key.pem" -CAcreateserial \
  -copy_extensions copy \
  -out "$CERT_DIR/simulated-upstream-cert.pem" >/dev/null 2>&1
python3 .github/e2e/simulated_upstream.py \
  --cert "$CERT_DIR/simulated-upstream-cert.pem" \
  --key "$CERT_DIR/simulated-upstream-key.pem" \
  --port "$SIMULATED_PORT" \
  --capture-log "$ARTIFACTS_DIR/captured-requests.log" >"$ARTIFACTS_DIR/simulated-upstream.log" 2>&1 &
SIMULATED_UPSTREAM_PID=$!

export NODE_EXTRA_CA_CERTS="$CA_CERT"
export HTTP_PROXY="http://127.0.0.1:2080"
export HTTPS_PROXY="$HTTP_PROXY"
export NO_PROXY="127.0.0.1,localhost"
export no_proxy="$NO_PROXY"
export OPENAI_COMPLETIONS_API_KEY="modeltap-e2e"
export OPENAI_COMPLETIONS_BASE_URL="https://$SIMULATED_HOST:$SIMULATED_PORT/v1"
export OPENAI_COMPLETIONS_MODEL="simulated-model"
export OPENAI_RESPONSES_API_KEY="$OPENAI_COMPLETIONS_API_KEY"
export OPENAI_RESPONSES_BASE_URL="$OPENAI_COMPLETIONS_BASE_URL"
export OPENAI_RESPONSES_MODEL="$OPENAI_COMPLETIONS_MODEL"
export ANTHROPIC_API_KEY="$OPENAI_COMPLETIONS_API_KEY"
export ANTHROPIC_BASE_URL="https://$SIMULATED_HOST:$SIMULATED_PORT"
export ANTHROPIC_MODEL="$OPENAI_COMPLETIONS_MODEL"
export GEMINI_API_KEY="$OPENAI_COMPLETIONS_API_KEY"
export GEMINI_BASE_URL="https://$SIMULATED_HOST:$SIMULATED_PORT"
export GEMINI_MODEL="$OPENAI_COMPLETIONS_MODEL"

export E2E_CONFIG_PATH="$CONFIG_PATH"
python3 - <<'PY'
import os
from pathlib import Path
from urllib.parse import urlparse

base_urls = [os.environ["OPENAI_COMPLETIONS_BASE_URL"]]
hosts = []
for base_url in base_urls:
    host = urlparse(base_url).hostname
    if not host:
        raise SystemExit(f"invalid base URL: {base_url!r}")
    if host not in hosts:
        hosts.append(host)
if os.environ["SIMULATED_HOST"] not in hosts:
    hosts.append(os.environ["SIMULATED_HOST"])

Path(os.environ["E2E_CONFIG_PATH"]).write_text(
    "proxy:\n"
    "  listen: 127.0.0.1:2080\n"
    "tls:\n"
    f"  ca_cert_file: {os.environ['CA_CERT']}\n"
    f"  ca_key_file: {os.environ['CERT_DIR']}/modeltap-ca-key.pem\n"
    "telemetry:\n"
    "  otlp:\n"
    "    endpoint: http://127.0.0.1:4318\n"
    "    service_name: modeltap-agent-e2e\n"
    "sites:\n"
    "  - id: e2e\n"
    "    hosts:\n"
    + "".join(f"      - {host}\n" for host in hosts)
    + "pricing:\n"
    "  timezone: UTC\n"
)
PY

OTEL_METRIC_EXPORT_INTERVAL=1000 ./target/debug/modeltap run --config "$CONFIG_PATH" \
  >"$ARTIFACTS_DIR/modeltap.log" 2>&1 &
MODEL_TAP_PID=$!
trap 'kill "$MODEL_TAP_PID" "$SIMULATED_UPSTREAM_PID" 2>/dev/null || true; docker logs modeltap-e2e-otel >"$ARTIFACTS_DIR/otel-collector.log" 2>&1 || true' EXIT

for i in $(seq 1 30); do
  if curl --silent --max-time 1 --proxy "" --connect-timeout 1 http://127.0.0.1:2080 >/dev/null 2>&1; then
    echo "modeltap proxy ready after ${i}s"
    break
  fi
  if [[ $i -eq 30 ]]; then
    echo "ERROR: modeltap proxy did not become ready after 30s" >&2
    echo "--- modeltap.log ---" >&2
    cat "$ARTIFACTS_DIR/modeltap.log" >&2
    exit 1
  fi
  sleep 1
done

export XDG_CONFIG_HOME="$RUNNER_TEMP/opencode-config"
mkdir -p "$XDG_CONFIG_HOME/opencode"
export E2E_OPENCODE_CONFIG="$XDG_CONFIG_HOME/opencode/opencode.json"
python3 - <<'PY'
import json
import os
from pathlib import Path

Path(os.environ["E2E_OPENCODE_CONFIG"]).write_text(json.dumps({
    "$schema": "https://opencode.ai/config.json",
    "provider": {
        "openai": {
            "options": {
                "baseURL": os.environ["OPENAI_COMPLETIONS_BASE_URL"],
                "apiKey": os.environ["OPENAI_COMPLETIONS_API_KEY"],
            },
            "models": {os.environ["OPENAI_COMPLETIONS_MODEL"]: {"name": "E2E model"}},
        },
    },
}))
PY
timeout 180s opencode run --print-logs --model "openai/$OPENAI_COMPLETIONS_MODEL" \
  --format json --auto "Reply exactly E2E_OK." >"$ARTIFACTS_DIR/opencode.json" 2>&1

export PI_CODING_AGENT_DIR="$RUNNER_TEMP/pi-agent"
timeout 180s pi --extension .github/e2e/pi-openai-provider.ts \
  --provider e2e-openai --model "$OPENAI_COMPLETIONS_MODEL" --no-tools --no-session \
  --print "Reply exactly E2E_OK." >"$ARTIFACTS_DIR/pi.txt"

timeout 180s pi \
  --extension "$(npm root -g)/oh-my-pi/dist/extension.js" \
  --extension .github/e2e/pi-openai-provider.ts \
  --provider e2e-openai --model "$OPENAI_COMPLETIONS_MODEL" --no-tools --no-session \
  --print "Reply exactly E2E_OK." >"$ARTIFACTS_DIR/oh-my-pi.txt"

export COPILOT_PROVIDER_BASE_URL="$OPENAI_COMPLETIONS_BASE_URL"
export COPILOT_PROVIDER_API_KEY="$OPENAI_COMPLETIONS_API_KEY"
export COPILOT_PROVIDER_TYPE=openai
export COPILOT_MODEL="$OPENAI_COMPLETIONS_MODEL"
export COPILOT_OFFLINE=true
timeout 180s copilot --allow-all-tools --prompt "Reply exactly E2E_OK." \
  >"$ARTIFACTS_DIR/copilot.txt"

export OPENAI_API_KEY="$OPENAI_COMPLETIONS_API_KEY"
export OPENAI_BASE_URL="$OPENAI_COMPLETIONS_BASE_URL"
export OPENAI_MODEL="$OPENAI_COMPLETIONS_MODEL"
timeout 180s qwen --model "$OPENAI_COMPLETIONS_MODEL" --prompt "Reply exactly E2E_OK." \
  >"$ARTIFACTS_DIR/qwen.txt"

export CODEX_HOME="$RUNNER_TEMP/codex-home"
mkdir -p "$CODEX_HOME"
export E2E_CODEX_CONFIG="$CODEX_HOME/config.toml"
python3 - <<'PY'
import json
import os
from pathlib import Path

base_url = json.dumps(os.environ["OPENAI_RESPONSES_BASE_URL"])
Path(os.environ["E2E_CODEX_CONFIG"]).write_text(
    "model_provider = \"e2e_responses\"\n"
    "[model_providers.e2e_responses]\n"
    "name = \"E2E OpenAI Responses\"\n"
    f"base_url = {base_url}\n"
    "env_key = \"OPENAI_RESPONSES_API_KEY\"\n"
    "wire_api = \"responses\"\n"
)
PY
timeout 180s codex exec --ephemeral --skip-git-repo-check --sandbox read-only \
  --model "$OPENAI_RESPONSES_MODEL" "Reply exactly E2E_OK." </dev/null \
  >"$ARTIFACTS_DIR/codex.txt"

timeout 180s claude --bare --print --no-session-persistence \
  --max-budget-usd 0.10 --model "$ANTHROPIC_MODEL" "Reply exactly E2E_OK." </dev/null \
  >"$ARTIFACTS_DIR/claude.txt"

export GOOGLE_GEMINI_BASE_URL="$GEMINI_BASE_URL"
export GEMINI_TELEMETRY_ENABLED=false
export GEMINI_CLI_TRUST_WORKSPACE="true"
mkdir -p "$HOME/.gemini"
cat > "$HOME/.gemini/settings.json" << 'EOF'
{
  "security": {
    "auth": {
      "selectedType": "gemini-api-key"
    }
  }
}
EOF
timeout 180s gemini --yolo --model "$GEMINI_MODEL" \
  --prompt "Reply exactly E2E_OK." </dev/null >"$ARTIFACTS_DIR/gemini.txt" || {
    cp /tmp/gemini-client-error-*.json "$ARTIFACTS_DIR/" 2>/dev/null || true
    exit 1
  }
python3 .github/e2e/simulate_agents.py \
  --url "https://$SIMULATED_HOST:$SIMULATED_PORT/v1/chat/completions" \
  --proxy "$HTTP_PROXY" >"$ARTIFACTS_DIR/simulated-agents.txt"

python3 .github/e2e/assert_metrics.py
curl --fail --silent --show-error http://127.0.0.1:9464/metrics >"$ARTIFACTS_DIR/metrics.prom"

echo "=== CAPTURED AGENT HTTP USER-AGENTS ==="
python3 - <<'PY'
import json, os
from pathlib import Path
log_file = Path(os.environ["ARTIFACTS_DIR"]) / "captured-requests.log"
if log_file.exists():
    seen = set()
    for line in log_file.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        entry = json.loads(line)
        key = (entry["method"], entry["path"], entry["user_agent"])
        if key not in seen:
            seen.add(key)
            print(f"{entry['method']} {entry['path']} -> User-Agent: {entry['user_agent']}")
PY
