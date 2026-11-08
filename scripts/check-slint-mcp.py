#!/usr/bin/env -S uv run --no-project python
"""Exercise the actual Rust canvas demo through its embedded Slint MCP server.

Build with SLINT_EMIT_DEBUG_INFO=1 and --features slint,slint/mcp first.
The child runs headless, changes only its disposable demo scene, and stops on
exit. JSON read-backs and PNGs go to --artifacts for further visual inspection.
Uses only Python's standard library; run with uv run --no-project python.
"""

import argparse
import base64
import json
import os
import re
import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--artifacts", type=Path, required=True)
args = parser.parse_args()
args.artifacts = args.artifacts.resolve()
args.artifacts.mkdir(parents=True, exist_ok=True)
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
endpoint = f"http://127.0.0.1:{port}/mcp"
env = os.environ | {
    "SLINT_MCP_HOST": "127.0.0.1",
    "SLINT_MCP_PORT": str(port),
    "SLINT_BACKEND": "headless-software",
    "SLINT_SCALE_FACTOR": "1",
}
env.pop("DISPLAY", None)
env.pop("WAYLAND_DISPLAY", None)
env.pop("SLINT_TEST_SERVER", None)
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
counter = 0


def rpc(method, params=None):
    global counter
    counter += 1
    data = json.dumps(
        {"jsonrpc": "2.0", "id": counter, "method": method, "params": params or {}}
    ).encode()
    request = urllib.request.Request(
        endpoint,
        data=data,
        headers={
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
        },
    )
    with opener.open(request, timeout=10) as response:
        result = json.load(response)
    if "error" in result:
        raise RuntimeError(result["error"])
    return result["result"]


def tool(name, arguments=None):
    result = rpc("tools/call", {"name": name, "arguments": arguments or {}})
    if result.get("isError"):
        raise RuntimeError(result)
    if name == "take_screenshot":
        return result
    return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))


def save(name, data):
    (args.artifacts / name).write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n"
    )


with (args.artifacts / "app.log").open("w") as log:
    process = subprocess.Popen(
        [str(args.binary.resolve()), "--no-stress"],
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    try:
        deadline = time.monotonic() + 25
        while True:
            if process.poll() is not None:
                raise RuntimeError(
                    f"Application exited: {process.returncode}; see {log.name}"
                )
            try:
                initialized = rpc(
                    "initialize",
                    {
                        "protocolVersion": "2025-06-18",
                        "capabilities": {},
                        "clientInfo": {"name": "canvas-check", "version": "1"},
                    },
                )
                break
            except urllib.error.URLError:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)
        save("initialize.json", initialized)
        tools = rpc("tools/list")
        save("tools.json", tools)
        windows = tool("list_windows")
        save("windows.json", windows)
        window = windows["windowHandles"][0]
        properties = tool("get_window_properties", {"windowHandle": window})
        save("window.json", properties)
        root = properties["rootElementHandle"]
        tree = tool("get_element_tree", {"elementHandle": root, "maxElements": 100})
        save("before.tree.json", tree)

        def screenshot(name):
            shot = tool("take_screenshot", {"windowHandle": window})
            images = [block for block in shot["content"] if block["type"] == "image"]
            if len(images) != 1:
                raise RuntimeError(f"Expected one screenshot, got {len(images)}")
            (args.artifacts / f"{name}.png").write_bytes(
                base64.b64decode(images[0]["data"])
            )

        def by_id(element_id):
            return tool(
                "find_elements_by_id",
                {"windowHandle": window, "elementsId": element_id},
            )["elementHandles"][0]

        status = by_id("DemoWindow::status-text")
        canvas = next(
            e["handle"]
            for e in tree["elements"]
            if any(t["typeName"] == "TouchArea" for t in e["typeNamesAndIds"])
        )

        def label():
            return tool("get_element_properties", {"elementHandle": status})[
                "accessibleLabel"
            ]

        def wait_for(predicate):
            deadline = time.monotonic() + 10
            while True:
                value = label()
                if predicate(value):
                    return value
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"State did not change as expected: {value}")
                time.sleep(0.05)

        def scale(value):
            return float(re.search(r"([\d.]+) px/mm", value)[1])

        def key(text):
            tool("dispatch_key_event", {"windowHandle": window, "text": text})

        before = label()
        screenshot("before")
        tool("start_event_recording")
        tool("scroll_element", {"elementHandle": canvas, "deltaY": 120})
        zoomed = wait_for(lambda s: scale(s) > scale(before))
        screenshot("zoomed")
        key("f")
        wait_for(lambda s: scale(s) == scale(before))
        key("m")
        time.sleep(0.1)
        screenshot("mirrored")
        key("m")
        time.sleep(0.1)
        width = properties["size"]["width"] / properties.get("scaleFactor", 1)
        height = properties["size"]["height"] / properties.get("scaleFactor", 1)
        tool(
            "drag_element",
            {"elementHandle": canvas, "target": {"x": width * 0.82, "y": height * 0.9}},
        )
        selected = wait_for(lambda s: bool(re.search(r"[1-9]\d* items selected", s)))
        screenshot("selected")
        count = int(re.search(r"(\d+) items selected", selected)[1])
        key("\u007f")
        deleted = wait_for(lambda s: f"removed {count} items" in s)
        if int(deleted.split()[0]) != int(before.split()[0]) - count:
            raise RuntimeError(f"Unexpected deletion count: {deleted}")
        screenshot("deleted")
        key("m")
        time.sleep(0.1)
        key("m")
        time.sleep(0.1)
        screenshot("deleted-full")
        events = tool("stop_event_recording")
        save("events.json", events)
        if int(events.get("unknownEventCount", 0)) or int(
            events.get("droppedCount", 0)
        ):
            raise RuntimeError("The event log lost events")
        save(
            "after.tree.json",
            tool("get_element_tree", {"elementHandle": root, "maxElements": 100}),
        )
        summary = {
            "tool_count": len(tools["tools"]),
            "before": before,
            "zoomed": zoomed,
            "selected": selected,
            "deleted": deleted,
            "recorded_events": len(events.get("events", [])),
        }
        save("summary.json", summary)
        print(json.dumps(summary, ensure_ascii=False))
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
