"""Time cold and warm checked routing on the original ESP-PD-Sink fixture."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
CASES = {
    "oled": ("routing_copper_prepare", HERE / "../../candidates/oled.json"),
    "builtin": ("routing_region_prepare", HERE / "../release/builtin-input.json"),
}
STAGE = re.compile(rb"routing_stage (\w+) (\d+) us")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def manifest(folder):
    return {
        str(path.relative_to(folder)): digest(path)
        for path in sorted(folder.rglob("*"))
        if path.is_file()
    }


def run_sample(binary, source, output, case, condition, number):
    tool, request_file = CASES[case]
    request = json.loads(request_file.read_text())
    sample = output / f"{case}-{condition}-{number:02}"
    sample.mkdir()
    project = sample / "project"
    shutil.copytree(source, project)
    original = manifest(project)
    log_path = sample / "server.log"
    with (
        log_path.open("wb") as log,
        (sample / "rpc.jsonl").open("w") as records,
        subprocess.Popen(
            [str(binary), "--project", str(project / "ESP-PD-Sink.lpp")],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=log,
            text=True,
            env={
                **os.environ,
                "RUST_LOG": "librepcb_editor::routing_service=debug,"
                "librepcb_editor::commands::autoroute=debug,librepcb_core::project::board::drc=debug",
            },
        ) as proc,
    ):
        sequence = 0

        def rpc(method, params):
            nonlocal sequence
            sequence += 1
            req = {"jsonrpc": "2.0", "id": sequence, "method": method, "params": params}
            start = log_path.stat().st_size
            now = time.perf_counter_ns()
            proc.stdin.write(json.dumps(req) + "\n")
            proc.stdin.flush()
            for line in proc.stdout:
                reply = json.loads(line)
                if reply.get("id") == sequence:
                    break
            else:
                raise RuntimeError(f"MCP exited: {log_path}")
            elapsed = (time.perf_counter_ns() - now) / 1e9
            with log_path.open("rb") as source_log:
                source_log.seek(start)
                stages = {
                    stage.decode(): int(us) / 1e6
                    for stage, us in STAGE.findall(
                        source_log.read(log_path.stat().st_size - start)
                    )
                }
            timing = {"seconds": elapsed, "stages_seconds": stages}
            records.write(
                json.dumps({"request": req, "response": reply, **timing}) + "\n"
            )
            records.flush()
            return reply["result"], timing

        def call(name, args=None, *, error=None):
            reply, timing = rpc("tools/call", {"name": name, "arguments": args or {}})
            envelope = reply["structuredContent"]
            if error:
                assert reply.get("isError") and envelope["error"]["kind"] == error, (
                    envelope
                )
                return envelope["error"], timing
            assert not reply.get("isError"), envelope
            return envelope["result"], timing

        try:
            rpc(
                "initialize",
                {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "foilfern-p0-replay", "version": "1"},
                },
            )
            proc.stdin.write(
                json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"})
                + "\n"
            )
            proc.stdin.flush()
            info, _ = call("server_info")
            assert info["build"]["cargo_profile"] == "release", info
            assert info["build"]["cargo_opt_level"] == "3", info
            assert info["build"]["source_commit"], info
            board, _ = call("board_get")
            history, _ = call("history")
            cycles = []
            for reused in [False, True] if condition == "warm" else [False]:
                prepared, timing = call(tool, request)
                assert prepared["baseline_reused"] is reused, prepared
                validation = prepared["validation"]
                assert (
                    validation["before"]["complete"] and validation["after"]["complete"]
                )
                assert (
                    validation["netlist_preserved"]
                    and validation["new_error_count"] == 0
                )
                assert (
                    validation["before"]["unrouted"] == 0
                    and prepared["mutation_count"] > 0
                )
                if case == "oled":
                    assert (
                        validation["acceptable"]
                        and validation["after"]["unrouted"] == 0
                    )
                else:
                    assert (
                        not validation["acceptable"]
                        and validation["after"]["unrouted"] == 1
                    )
                    assert [
                        net["name"] for net in validation["unrouted_regression_details"]
                    ] == ["GND"]
                    assert (
                        prepared["routing"]["routed"] == 2
                        and prepared["routing"]["unrouted"] == 0
                    )
                required = {
                    "snapshot",
                    "source_checks",
                    "candidate_work",
                    "candidate_checks",
                }
                if case == "builtin":
                    required |= {"extraction", "search"}
                assert required <= timing["stages_seconds"].keys(), timing
                args = {
                    "candidate": prepared["candidate"],
                    "expected_revision": prepared["base_revision"],
                }
                preview, preview_timing = call(
                    "routing_preview", {"candidate": args["candidate"], "limit": 200}
                )
                assert not preview["stale"] and preview["next_offset"] is None
                assert preview["validation"] == validation
                cycles.append(
                    {
                        "prepare": timing,
                        "preview": preview_timing,
                        "baseline_reused": reused,
                    }
                )
                if condition == "warm" and not reused:
                    call("routing_discard", args)
            assert call("history")[0] == history
            assert call("board_get")[0] == board
            if case == "oled":
                applied, apply_timing = call("routing_apply", args)
                assert applied["applied"] and applied["validation"] == validation
                assert "apply_checks" in apply_timing["stages_seconds"]
                assert call("history")[0]["done"] == history["done"] + 1
                after, _ = call("board_get")
                assert len(after["traces"]) == 996 and len(after["vias"]) == 143
            else:
                _, apply_timing = call("routing_apply", args, error="refused")
                assert "apply_checks" not in apply_timing["stages_seconds"]
                assert call("history")[0] == history and call("board_get")[0] == board
                call("routing_discard", args)
            call("project_close", {"discard_changes": True})
            proc.stdin.close()
            assert proc.wait(timeout=10) == 0
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=10)
    assert manifest(project) == original
    shutil.rmtree(project)
    return {
        "sample": sample.name,
        "case": case,
        "condition": condition,
        "build": info["build"],
        "cycles": cycles,
        "apply": apply_timing,
        "validation": validation,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument(
        "source", type=Path, help="Extracted foilfern/ directory from inputs.tar.gz"
    )
    parser.add_argument(
        "output", type=Path, help="New directory for logs and sample data"
    )
    parser.add_argument(
        "--samples", type=int, default=5, help="Sessions per case and condition"
    )
    options = parser.parse_args()
    if options.samples < 1:
        parser.error("--samples must be positive")
    options.output.mkdir(parents=True)
    context = {
        "binary": str(options.binary.resolve()),
        "binary_sha256": digest(options.binary),
        "source_files": manifest(options.source),
        "requests": {case: digest(path) for case, (_, path) in CASES.items()},
    }
    (options.output / "context.json").write_text(json.dumps(context, indent=2) + "\n")
    with (options.output / "samples.jsonl").open("w") as results:
        for number in range(1, options.samples + 1):
            conditions = ("cold", "warm") if number % 2 else ("warm", "cold")
            for case in CASES:
                for condition in conditions:
                    result = run_sample(
                        options.binary.resolve(),
                        options.source,
                        options.output,
                        case,
                        condition,
                        number,
                    )
                    results.write(json.dumps(result) + "\n")
                    results.flush()
                    print(
                        result["sample"],
                        *(round(c["prepare"]["seconds"], 3) for c in result["cycles"]),
                        "apply",
                        round(result["apply"]["seconds"], 3),
                        flush=True,
                    )


if __name__ == "__main__":
    main()
