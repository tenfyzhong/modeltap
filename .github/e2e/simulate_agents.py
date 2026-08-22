#!/usr/bin/env python3
"""Send representative User-Agent and Cursor protocol requests through ModelTap."""

import argparse
import json
import ssl
import urllib.request


AGENTS = (
    ("claude_code", "claude-code/2.1.89 (cli)"), ("codex", "codex_cli_rs/1.0"),
    ("oh_my_pi", "oh-my-pi/1.0"), ("gemini_cli", "GeminiCLI/0.34.0/gemini-pro"),
    ("opencode", "OpenCode/1.0"), ("pi", "pi (linux 6.11; x64)"),
    ("github_copilot", "copilot/0.0.353 (linux)"), ("amazon_q", "AmazonQ-For-CLI/1.0"),
    ("roo_code", "RooCode/3.53.0"), ("qwen_code", "QwenCode/0.14.0 (linux; x64)"),
    ("factory_droid", "factory-cli/0.62.1"), ("crush", "Charm-Crush/0.1"),
    ("kiro", "kiro-ide/1.0"), ("qoder", "Qoder-Cli/1.0"),
    ("antigravity", "antigravity/2.0.1 linux/x64"),
)


def varint(value):
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def length_field(field, value):
    return varint((field << 3) | 2) + varint(len(value)) + value


def cursor_request():
    message = length_field(1, length_field(3, length_field(1, b"simulated-cursor-model")))
    return b"\x00" + len(message).to_bytes(4, "big") + message


parser = argparse.ArgumentParser()
parser.add_argument("--url", required=True)
parser.add_argument("--proxy", required=True)
args = parser.parse_args()
opener = urllib.request.build_opener(
    urllib.request.ProxyHandler({"https": args.proxy}),
    urllib.request.HTTPSHandler(context=ssl.create_default_context()),
)
for agent_cli, user_agent in AGENTS:
    request = urllib.request.Request(args.url, data=json.dumps({"model": "simulated-model", "messages": []}).encode(), headers={"Content-Type": "application/json", "User-Agent": user_agent}, method="POST")
    with opener.open(request, timeout=20) as response:
        response.read()
    print(f"simulated {agent_cli}")
request = urllib.request.Request(args.url, data=cursor_request(), headers={"Content-Type": "application/connect+proto", "User-Agent": "cursor/1.0"}, method="POST")
with opener.open(request, timeout=20) as response:
    response.read()
print("simulated cursor")
