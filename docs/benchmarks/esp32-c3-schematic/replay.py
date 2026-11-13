"""Replay the published ESP32-C3 schematic through a fresh stdio MCP session."""

import argparse
import base64
import hashlib
import json
import os
import subprocess
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
RESOURCE = "librepcb://examples/esp32-c3-devboard"
LIBRARIES = {
    "1b4c6aca-3c96-4e19-b384-fdb1bdeeb4fd": "C&K",
    "6ccc516c-21b7-4cd5-9cf2-7a04cfa361c6": "LibrePCB Connectors",
    "a9ddf0c6-9b1c-4730-b300-01b4f192ad40": "LibrePCB Base",
    "ada7156a-41a9-4a65-9a43-957210e40b32": "Espressif Systems",
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def library_hash(root):
    fingerprint = hashlib.sha256()
    for entry in sorted(path for path in root.rglob("*") if path.is_file()):
        relative = entry.relative_to(root)
        if ".git" not in relative.parts:
            fingerprint.update(f"{relative.as_posix()}\0{digest(entry)}\n".encode())
    return fingerprint.hexdigest()


class Mcp:
    def __init__(self, proc, output):
        self.proc = proc
        self.output = output
        self.sequence = 0
        self.calls = []
        self.rpc(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "schematic-replay", "version": "1"},
            },
        )
        self.send({"method": "notifications/initialized"})

    def send(self, request):
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", **request}) + "\n")
        self.proc.stdin.flush()

    def rpc(self, method, params):
        self.sequence += 1
        self.send({"id": self.sequence, "method": method, "params": params})
        for line in self.proc.stdout:
            reply = json.loads(line)
            if reply.get("id") == self.sequence:
                if "error" in reply:
                    raise RuntimeError(reply["error"])
                return reply["result"]
        raise RuntimeError("MCP exited. See server.log in the output directory.")

    def call(self, tool, args=None, image=None):
        start = time.monotonic()
        reply = self.rpc("tools/call", {"name": tool, "arguments": args or {}})
        elapsed = time.monotonic() - start
        result = reply["structuredContent"]
        if reply.get("isError") or result["outcome"] != "complete":
            raise RuntimeError(f"{tool}: {result}")
        if image:
            item = next(item for item in reply["content"] if item["type"] == "image")
            (self.output / image).write_bytes(base64.b64decode(item["data"]))
        self.calls.append({"tool": tool, "seconds": elapsed})
        return result["result"]


def check_equal(name, actual, expected):
    if actual != expected:
        raise RuntimeError(f"{name} differs: expected {expected!r}, got {actual!r}")


def semantics(page, netlist):
    return {
        "symbols": len(page["symbols"]),
        "segments": len(page["net_segments"]),
        "notes": len(page["texts"]),
        "nets": netlist["nets"],
        "no_connects": sorted(
            pin["pin"]
            for symbol in page["symbols"]
            for pin in symbol["pins"]
            if pin["no_connect"]
        ),
    }


def replay(mcp, output):
    info = mcp.call("server_info")
    text = mcp.rpc("resources/read", {"uri": RESOURCE})["contents"][0]["text"]
    request = output / "document.json"
    request.write_bytes(text.encode())
    document = json.loads(text)
    expected = HERE / "expected.json"
    mcp.call("library_rescan")
    project = mcp.call(
        "project_create",
        {"name": "ESP32-C3 Replay", "create_schematic": False, "create_board": False},
    )
    before = mcp.call("schematic_list")
    check_equal("initial pages", before["schematics"], [])
    preview = mcp.call("design_preview", document, image="preview.png")
    check_equal(
        "ERC diff", preview["erc"], {"introduced": [], "inherited": [], "resolved": []}
    )
    check_equal("live pages during preview", mcp.call("schematic_list"), before)
    mcp.call("design_apply", {"preview": preview["preview"]})
    page = mcp.call("schematic_get")
    netlist = mcp.call("netlist")
    semantic = semantics(page, netlist)
    check_equal("schematic semantics", semantic, json.loads(expected.read_text()))
    check_equal("Foilfern ERC", mcp.call("erc_run")["messages"], [])
    mcp.call("undo")
    check_equal("undo", mcp.call("schematic_list"), before)
    mcp.call("redo")
    check_equal("redo", mcp.call("schematic_get"), page)
    mcp.call(
        "symbol_text_move",
        {"component": "C3", "text": "name", "position": {"x": 15.24, "y": 29.45}},
    )
    edited = mcp.call("schematic_get")
    mcp.call("project_save")
    mcp.call("project_close")
    mcp.call("project_open", {"path": project["path"]})
    repeated = mcp.call("design_preview", document)
    check_equal("replay diff", repeated["diff"], [])
    check_equal(
        "empty apply",
        mcp.call("design_apply", {"preview": repeated["preview"]})["operation_count"],
        0,
    )
    check_equal("reopened page with manual edit", mcp.call("schematic_get"), edited)
    check_equal("reopened netlist", mcp.call("netlist"), netlist)
    mcp.call("project_close")
    return {
        "build": info["build"],
        "api_capability_version": info["api_capability_version"],
        "document_sha256": digest(request),
        "expected_sha256": digest(expected),
        "project": project["path"],
        "semantic": semantic,
        "preview_changes": len(preview["diff"]),
        "calls": mcp.calls,
    }


def upstream_check(cli, project, output):
    if cli is None:
        return {"status": "not_run"}
    env = {**os.environ, "QT_QPA_PLATFORM": "offscreen", "LC_ALL": "C", "LANGUAGE": "C"}
    version = subprocess.check_output([cli, "--version"], text=True, env=env).strip()
    check = subprocess.run(
        [cli, "open-project", project, "--erc"],
        check=False,
        text=True,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    (output / "upstream-erc.log").write_text(check.stdout)
    if check.returncode or "Non-approved messages: 0" not in check.stdout:
        raise RuntimeError("Upstream ERC failed. See upstream-erc.log.")
    return {"status": "pass", "version": version}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--libraries", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--upstream-cli", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True)
    binary = args.binary.resolve(strict=True)
    sources = {
        name: (args.libraries / f"{uuid}.lplib").resolve(strict=True)
        for uuid, name in LIBRARIES.items()
    }
    with (
        (output / "server.log").open("w") as log,
        subprocess.Popen(
            [str(binary)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=log,
            text=True,
        ) as proc,
    ):
        try:
            mcp = Mcp(proc, output)
            workspace = mcp.call(
                "workspace_create", {"path": str(output / "workspace")}
            )
            local = Path(workspace["libraries_path"]) / "local"
            local.mkdir(parents=True, exist_ok=True)
            for source in sources.values():
                (local / source.name).symlink_to(source, target_is_directory=True)
            report = replay(mcp, output)
        finally:
            proc.stdin.close()
    report["binary_sha256"] = digest(binary)
    report["library_sha256"] = {
        name: library_hash(path) for name, path in sources.items()
    }
    cli = args.upstream_cli.resolve(strict=True) if args.upstream_cli else None
    report["upstream_erc"] = upstream_check(cli, report["project"], output)
    (output / "report.json").write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    )
    print(output / "report.json")


if __name__ == "__main__":
    main()
