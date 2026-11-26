#!/usr/bin/env python3
"""Minimal MCP stdio server used by the Rust tests.

Modes via MOCK_MCP_MODE:
  normal    initialize, tools/list, tools/call
  malformed emit an invalid JSON line for tools/list
  crash     exit before reading input
  silent    read but never answer
"""

import json
import os
import sys

MODE = os.environ.get("MOCK_MCP_MODE", "normal")


def send(payload):
    sys.stdout.write(json.dumps(payload, separators=(",", ":")) + "\n")
    sys.stdout.flush()


if MODE == "crash":
    sys.exit(1)

print("mock mcp ready", file=sys.stderr, flush=True)

resource_page = 0
prompt_page = 0

for raw in sys.stdin:
    raw = raw.strip()
    if not raw:
        continue
    try:
        request = json.loads(raw)
    except json.JSONDecodeError:
        continue
    method = request.get("method")
    request_id = request.get("id")

    if MODE == "silent":
        continue

    if method is None and request_id == 900:
        print("sampling response: " + json.dumps(request), file=sys.stderr, flush=True)
        continue

    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {
                        "tools": {},
                        "sampling": {},
                        "resources": {"subscribe": True},
                    },
                    "serverInfo": {"name": "mock", "version": "1.0"},
                },
            }
        )
        if MODE == "sampling":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": 900,
                    "method": "sampling/createMessage",
                    "params": {
                        "messages": [
                            {"role": "user", "content": {"type": "text", "text": "sample me"}}
                        ]
                    },
                }
            )
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        if MODE == "oversized":
            big = "x" * (8 * 1024 * 1024 + 100)
            sys.stdout.write(
                '{"jsonrpc":"2.0","id":'
                + str(request_id)
                + ',"result":{"tools":[{"name":"big","description":"'
                + big
                + '"}]}}\n'
            )
            sys.stdout.flush()
            continue
        if MODE == "malformed":
            sys.stdout.write("this is not json\n")
            sys.stdout.flush()
            continue
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "tools": [
                        {
                            "name": "echo",
                            "description": "Echo text",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"text": {"type": "string"}},
                            },
                        },
                        {
                            "name": "add",
                            "description": "Add two integers",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "a": {"type": "integer"},
                                    "b": {"type": "integer"},
                                },
                            },
                        },
                        {
                            "name": "read_text_file",
                            "description": "Read a text file",
                            "inputSchema": {
                                "type": "object",
                                "properties": {"path": {"type": "string"}},
                                "required": ["path"],
                            },
                        },
                    ]
                },
            }
        )
    elif method == "resources/list":
        params = request.get("params") or {}
        if MODE == "paginated" and not params.get("cursor"):
            resources = [
                {"uri": "mock://one", "name": "One", "description": "First page", "mimeType": "text/plain"}
            ]
            next_cursor = "page-2"
        else:
            resources = [
                {
                    "uri": "mock://readme",
                    "name": "README",
                    "description": "Mock resource",
                    "mimeType": "text/plain",
                }
            ]
            next_cursor = None
            if MODE == "paginated":
                resources = [
                    {"uri": "mock://two", "name": "Two", "description": "Second page", "mimeType": "text/plain"}
                ]
        result = {"resources": resources}
        if next_cursor:
            result["nextCursor"] = next_cursor
        send({"jsonrpc": "2.0", "id": request_id, "result": result})
    elif method == "resources/templates/list":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "resourceTemplates": [
                        {
                            "uriTemplate": "mock://item/{id}",
                            "name": "Item",
                            "description": "Parameterized mock resource",
                            "mimeType": "text/plain",
                        }
                    ]
                },
            }
        )
    elif method == "resources/subscribe":
        uri = (request.get("params") or {}).get("uri", "mock://readme")
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
        send(
            {
                "jsonrpc": "2.0",
                "method": "notifications/resources/updated",
                "params": {"uri": uri},
            }
        )
    elif method == "resources/unsubscribe":
        send({"jsonrpc": "2.0", "id": request_id, "result": {}})
    elif method == "resources/read":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "contents": [
                        {
                            "uri": "mock://readme",
                            "mimeType": "text/plain",
                            "text": "hello resource",
                        }
                    ]
                },
            }
        )
    elif method == "prompts/list":
        params = request.get("params") or {}
        if MODE == "paginated" and not params.get("cursor"):
            prompts = [{"name": "first", "description": "First page"}]
            next_cursor = "page-2"
        else:
            prompts = [
                {
                    "name": "review",
                    "description": "Review the change",
                    "arguments": [{"name": "path", "required": True}],
                }
            ]
            next_cursor = None
            if MODE == "paginated":
                prompts = [{"name": "second", "description": "Second page"}]
        result = {"prompts": prompts}
        if next_cursor:
            result["nextCursor"] = next_cursor
        send({"jsonrpc": "2.0", "id": request_id, "result": result})
    elif method == "prompts/get":
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "description": "Review the change",
                    "messages": [
                        {"role": "user", "content": {"type": "text", "text": "review it"}}
                    ],
                },
            }
        )
    elif method == "tools/call":
        params = request.get("params") or {}
        name = params.get("name")
        arguments = params.get("arguments") or {}
        print(f"mock call {name}", file=sys.stderr, flush=True)
        if name == "echo":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "content": [
                            {"type": "text", "text": str(arguments.get("text", ""))}
                        ],
                        "isError": False,
                    },
                }
            )
        elif name == "add":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": str(arguments.get("a", 0) + arguments.get("b", 0)),
                            }
                        ],
                        "isError": False,
                    },
                }
            )
        elif name == "read_text_file":
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "content": [
                            {"type": "text", "text": str(arguments.get("path", ""))}
                        ],
                        "isError": False,
                    },
                }
            )
        else:
            send(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32602, "message": "unknown tool"},
                }
            )
    elif request_id is not None:
        send(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "method not found"},
            }
        )
