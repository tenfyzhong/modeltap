#!/usr/bin/env python3
"""HTTPS upstream used to exercise simulated agent requests."""

import argparse
import json
import ssl
from http.server import BaseHTTPRequestHandler, HTTPServer


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
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if self.headers.get("Content-Type", "").startswith("application/connect+proto"):
            body, content_type = cursor_response(), "application/connect+proto"
        else:
            body, content_type = json.dumps({"model": "simulated-model", "usage": {"prompt_tokens": 3, "completion_tokens": 5}}).encode(), "application/json"
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        return


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
