"""Replay prepare/discard on an unchanged source, then verify non-empty apply.

Usage: benchmark.py BINARY LABEL SOURCE REQUEST OUTPUT_DIR [TOOL]
Build both binaries with cargo build --locked --release -p librepcb-mcp --bin librepcb-mcp.
The source is copied; no native files are saved. Runs must be serial, without builds.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

binary, label, source, request_path, output, *tool_arg = sys.argv[1:]
tool = tool_arg[0] if tool_arg else 'routing_copper_prepare'
root = Path(output).resolve()
root.mkdir(parents=True, exist_ok=True)
folder = root / ('project-' + label)
if folder.exists():
    raise RuntimeError(f'Refusing to reuse existing copy: {folder}')
shutil.copytree(source, folder, ignore=shutil.ignore_patterns('output', '*.lock', '*.user.lp'))

def native_manifest(path):
    return {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(path.rglob('*')) if p.is_file()
            and not p.name.endswith(('.lock', '.user.lp')) and 'output' not in p.parts}

native_before = native_manifest(folder)
(root / (label + '-native-manifest.json')).write_text(json.dumps(native_before, indent=2))
log_path = root / (label + '.stderr')
records = []
sequence = 0
with log_path.open('w') as log:
    proc = subprocess.Popen([binary, '--project', str(folder / 'ESP-PD-Sink.lpp')],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1,
        env=dict(os.environ, RUST_LOG='librepcb_core::project::board::drc=debug'))

    def rpc(method, params):
        global sequence
        sequence += 1
        request = {'jsonrpc': '2.0', 'id': sequence, 'method': method, 'params': params}
        log_start = log_path.stat().st_size
        started = time.perf_counter()
        proc.stdin.write(json.dumps(request) + '\n')
        proc.stdin.flush()
        while True:
            line = proc.stdout.readline()
            if not line:
                raise RuntimeError(f'Server exited; inspect {log_path}')
            reply = json.loads(line)
            if reply.get('id') == sequence:
                break
        records.append({'request': request, 'response': reply, 'seconds': time.perf_counter() - started,
                        'drc_log_start': log_start, 'drc_log_end': log_path.stat().st_size})
        (root / (label + '-records.json')).write_text(json.dumps(records, indent=2))
        if 'error' in reply:
            raise RuntimeError(reply['error'])
        return reply['result']

    def call(name, args=None):
        reply = rpc('tools/call', {'name': name, 'arguments': args or {}})
        value = reply.get('structuredContent') or json.loads(reply['content'][0]['text'])
        assert not reply.get('isError') and not value.get('error'), value
        print(label, name, round(records[-1]['seconds'], 3), flush=True)
        return value['result']

    try:
        rpc('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
            'clientInfo': {'name': 'release-routing-benchmark', 'version': '1'}})
        proc.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n')
        proc.stdin.flush()
        for name in ['server_info', 'project_summary', 'schematic_list', 'component_list', 'board_get']:
            call(name)
        initial_history = call('history')
        request = json.loads(Path(request_path).read_text())
        times, validations, reuse, revisions = [], [], [], []
        for _ in range(3):
            prepared = call(tool, request)
            times.append(records[-1]['seconds'])
            v = prepared['validation']
            assert v['acceptable'] and v['after']['unrouted'] == 0 and v['new_error_count'] == 0 and v['netlist_preserved'], v
            assert prepared['mutation_count'] > 0
            validations.append(v)
            reuse.append(prepared.get('baseline_reused', False))
            revisions.append(prepared['base_revision'])
            preview = call('routing_preview', {'candidate': prepared['candidate'], 'limit': 200})
            assert preview['next_offset'] is None and not preview['stale']
            call('routing_discard', {'candidate': prepared['candidate'], 'expected_revision': prepared['base_revision']})
        assert len(set(revisions)) == 1
        assert call('history') == initial_history, 'prepare/discard changed live history'
        # A separate fourth prepare verifies actual application, outside timed repeated samples.
        prepared = call(tool, request)
        verification_prepare_seconds = records[-1]['seconds']
        applied = call('routing_apply', {'candidate': prepared['candidate'], 'expected_revision': prepared['base_revision']})
        apply_seconds = records[-1]['seconds']
        assert applied['applied'] and applied['validation']['acceptable'] and applied['validation']['after']['unrouted'] == 0
        final_drc = call('drc_run')
        assert final_drc['errors'] == 3 and final_drc['warnings'] == 9, final_drc
        board = call('board_get')
        # Generated segment/junction/via/trace UUIDs differ. Keep endpoint kind, pad identity,
        # coordinates, nets, layers, widths, all via properties and component placement.
        def anchor(e):
            return {'kind': 'pad' if 'pad_uuid' in e else 'via' if 'via' in e else 'junction',
                    'pad': e.get('pad'), 'pad_uuid': e.get('pad_uuid'), 'position': e['position']}
        traces = sorted(json.dumps({'net': t['net'], 'layer': t['layer'], 'width': t['width'],
            'ends': sorted(json.dumps(anchor(t[e]), sort_keys=True) for e in ['from', 'to'])}, sort_keys=True)
            for t in board['traces'])
        vias = sorted(json.dumps({k: v for k, v in q.items() if k not in ['segment', 'uuid']}, sort_keys=True) for q in board['vias'])
        geometry = {'traces': traces, 'vias': vias, 'devices': board['devices'], 'planes': board['planes'],
                    'air_wires': board['air_wires']}
        geometry_sha = hashlib.sha256(json.dumps(geometry, sort_keys=True).encode()).hexdigest()
        peak = next(line for line in Path(f'/proc/{proc.pid}/status').read_text().splitlines() if line.startswith('VmHWM:'))
        call('project_close', {'discard_changes': True})
        proc.stdin.close()
        assert proc.wait(timeout=10) == 0
    finally:
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=10)
assert native_manifest(folder) == native_before, 'Native files changed'
log_bytes = log_path.read_bytes()
prepare_drc = [[float(x) / 1000 for x in re.findall(rb'DRC succeeded after (\d+) ms',
    log_bytes[r['drc_log_start']:r['drc_log_end']])]
    for r in records if r['request'].get('params', {}).get('name') == tool]
summary = {'prepare_drc_seconds': prepare_drc, 'label': label, 'binary': binary, 'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
    'prepare_seconds': times, 'baseline_reused': reuse, 'base_revisions': revisions,
    'verification_prepare_seconds': verification_prepare_seconds, 'verification_baseline_reused': prepared.get('baseline_reused', False),
    'apply_seconds': apply_seconds, 'validations': validations, 'applied_validation': applied['validation'],
    'final_drc': final_drc, 'geometry_sha256': geometry_sha, 'peak_process': peak, 'native_unchanged': True,
    'trace_count': len(traces), 'via_count': len(vias),
    'drc_core_seconds': [float(x) / 1000 for x in re.findall(r'DRC succeeded after (\d+) ms', log_path.read_text())]}
(root / (label + '-summary.json')).write_text(json.dumps(summary, indent=2))
print(json.dumps({k: v for k, v in summary.items() if k not in ['validations', 'applied_validation', 'final_drc']}, indent=2))
