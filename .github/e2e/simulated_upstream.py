#!/usr/bin/env python3
"""HTTPS upstream used to exercise simulated agent requests."""

import argparse
import json
import ssl
from http.server import BaseHTTPRequestHandler, HTTPServer
from urllib.parse import urlparse


def varint(value):
    result = bytearray()
    while value > 127:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def length_field(field, value):
    return varint((field << 3) | 2) + varint(len(value)) + value


def cursor_response():
    token_delta = varint(8) + varint(7)
    return b"\x00" + len(length_field(1, length_field(8, token_delta))).to_bytes(4, "big") + length_field(1, length_field(8, token_delta))


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        self.close_connection = True
        body = json.dumps({"name": "models/simulated-model", "displayName": "Simulated Model"}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def do_POST(self):
        self.close_connection = True
        request_body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        path = urlparse(self.path).path
        if self.headers.get("Content-Type", "").startswith("application/connect+proto"):
            body, content_type = cursor_response(), "application/connect+proto"
        elif "/responses" in path:
            body = (
                b'data: {"type":"response.created","response":{"id":"resp_123","object":"response","status":"in_progress","model":"simulated-model","output":[]}}\n\n'
                b'data: {"type":"response.output_item.added","output_index":0,"item":{"id":"msg_123","type":"message","role":"assistant","status":"in_progress","content":[]}}\n\n'
                b'data: {"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"text","text":""}}\n\n'
                b'data: {"type":"response.text.delta","output_index":0,"content_index":0,"delta":"E2E_OK"}\n\n'
                b'data: {"type":"response.text.done","output_index":0,"content_index":0,"text":"E2E_OK"}\n\n'
                b'data: {"type":"response.content_part.done","output_index":0,"content_index":0,"part":{"type":"text","text":"E2E_OK"}}\n\n'
                b'data: {"type":"response.output_item.done","output_index":0,"item":{"id":"msg_123","type":"message","role":"assistant","status":"completed","content":[{"type":"text","text":"E2E_OK"}]}}\n\n'
                b'data: {"type":"response.completed","response":{"id":"resp_123","object":"response","status":"completed","model":"simulated-model","output":[{"id":"msg_123","type":"message","role":"assistant","status":"completed","content":[{"type":"text","text":"E2E_OK"}]}],"usage":{"input_tokens":3,"output_tokens":5,"total_tokens":8,"input_token_details":{"cached_tokens":0},"output_token_details":{"reasoning_tokens":0}}}}\n\n'
                b'data: [DONE]\n\n'
            )
            content_type = "text/event-stream"
        elif "/messages" in path:
            body = (
                b'event: message_start\n'
                b'data: {"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","model":"simulated-model","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":0}}}\n\n'
                b'event: content_block_start\n'
                b'data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}\n\n'
                b'event: content_block_delta\n'
                b'data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"E2E_OK"}}\n\n'
                b'event: content_block_stop\n'
                b'data: {"type":"content_block_stop","index":0}\n\n'
                b'event: message_delta\n'
                b'data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":5}}\n\n'
                b'event: message_stop\n'
                b'data: {"type":"message_stop"}\n\n'
            )
            content_type = "text/event-stream"
        elif "generateContent" in self.path:
            resp_data = {
                "candidates": [
                    {
                        "content": {
                            "parts": [{"text": "E2E_OK"}],
                            "role": "model"
                        },
                        "finishReason": "STOP"
                    }
                ],
                "usageMetadata": {
                    "promptTokenCount": 3,
                    "candidatesTokenCount": 5,
                    "totalTokenCount": 8
                }
            }
            if "alt=sse" in self.path or "streamGenerateContent" in self.path:
                body = b"data: " + json.dumps(resp_data).encode() + b"\r\n\r\n"
                content_type = "text/event-stream; charset=utf-8"
            else:
                body = json.dumps(resp_data).encode()
                content_type = "application/json"
        else:
            request = json.loads(request_body or b"{}")
            if request.get("stream"):
                body = (
                    b"data: {\"model\":\"simulated-model\",\"choices\":[{\"delta\":{\"content\":\"E2E_OK\"},\"finish_reason\":null}]}\n\n"
                    b"data: {\"model\":\"simulated-model\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":5}}\n\n"
                    b"data: [DONE]\n\n"
                )
                content_type = "text/event-stream"
            else:
                body, content_type = json.dumps({"model": "simulated-model", "choices": [{"message": {"content": "E2E_OK"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 5}}).encode(), "application/json"
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        if not ("generateContent" in self.path and ("alt=sse" in self.path or "streamGenerateContent" in self.path)):
            self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()
    def log_message(self, format, *args):
        super().log_message(format, *args)


parser = argparse.ArgumentParser()
parser.add_argument("--cert", required=True)
parser.add_argument("--key", required=True)
parser.add_argument("--port", type=int, required=True)
args = parser.parse_args()
server = HTTPServer(("127.0.0.1", args.port), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(args.cert, args.key)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
