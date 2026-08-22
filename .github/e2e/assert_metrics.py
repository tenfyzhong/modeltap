#!/usr/bin/env python3
"""Assert that ModelTap exported requests and tokens for each E2E agent."""

from __future__ import annotations

import re
import sys
import time
import urllib.request


METRICS_URL = "http://127.0.0.1:9464/metrics"
EXPECTED_AGENT_CLIS = (
    "claude_code", "codex", "oh_my_pi", "gemini_cli", "opencode", "pi",
    "github_copilot", "amazon_q", "roo_code", "qwen_code", "factory_droid",
    "crush", "kiro", "qoder", "antigravity", "cursor",
)

if len(sys.argv) != 1:
    raise SystemExit("assert_metrics.py does not accept arguments")


def samples(metrics: str, metric: str, agent: str) -> list[float]:
    pattern = re.compile(
        rf"^{re.escape(metric)}(?:_total)?\\{{[^}}]*agent_cli=\\\"{re.escape(agent)}\\\"[^}}]*\\}} ([0-9.eE+-]+)$"
    )
    return [float(match.group(1)) for line in metrics.splitlines() if (match := pattern.match(line))]


for attempt in range(60):
    try:
        with urllib.request.urlopen(METRICS_URL, timeout=5) as response:
            metrics = response.read().decode()
    except OSError:
        metrics = ""

    missing = [
        f"{agent}:{metric}"
        for agent in EXPECTED_AGENT_CLIS
        for metric in ("ai_proxy_requests", "ai_proxy_tokens")
        if sum(samples(metrics, metric, agent)) <= 0
    ]
    if not missing:
        break
    time.sleep(1)
else:
    print("Timed out waiting for expected ModelTap metrics:", ", ".join(missing), file=sys.stderr)
    print(metrics, file=sys.stderr)
    raise SystemExit(1)

for agent in EXPECTED_AGENT_CLIS:
    requests = sum(samples(metrics, "ai_proxy_requests", agent))
    tokens = sum(samples(metrics, "ai_proxy_tokens", agent))
    print(f"{agent}: requests={requests:g}, tokens={tokens:g}")
