"""Profile a deliberately rejected regional candidate without applying it.

Usage: profile_region.py BINARY LABEL SOURCE REQUEST OUTPUT_DIR
Three prepare/preview/discard cycles share one source/revision. The last apply
must refuse the known GND regression. Full baseline DRC and native hashes remain.
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

binary, label, source, request_path, output = sys.argv[1:]
root = Path(output).resolve()
folder = root / ('project-' + label)
shutil.copytree(source, folder, ignore=shutil.ignore_patterns('output', '*.lock', '*.user.lp'))
def native_manifest():
    return {str(p.relative_to(folder)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(folder.rglob('*')) if p.is_file() and not p.name.endswith(('.lock', '.user.lp')) and 'output' not in p.parts}
native_before = native_manifest()
log_path = root / (label + '.stderr')
records = []
sequence = 0
with log_path.open('w') as log:
    proc = subprocess.Popen([binary, '--project', str(folder / 'ESP-PD-Sink.lpp')], stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1,
        env=dict(os.environ, RUST_LOG='librepcb_core::project::board::drc=debug'))
    def rpc(method, params):
        global sequence
        sequence += 1
        request = {'jsonrpc': '2.0', 'id': sequence, 'method': method, 'params': params}
        log_start = log_path.stat().st_size
        start = time.perf_counter()
        proc.stdin.write(json.dumps(request) + '\n'); proc.stdin.flush()
        while True:
            line = proc.stdout.readline()
            if not line: raise RuntimeError(f'Server exited; inspect {log_path}')
            reply = json.loads(line)
            if reply.get('id') == sequence: break
        records.append({'request': request, 'response': reply, 'seconds': time.perf_counter() - start,
            'drc_log_start': log_start, 'drc_log_end': log_path.stat().st_size})
        (root / (label + '-records.json')).write_text(json.dumps(records, indent=2))
        assert 'error' not in reply, reply
        return reply['result']
    def call(name, args=None):
        reply = rpc('tools/call', {'name': name, 'arguments': args or {}})
        value = reply.get('structuredContent') or json.loads(reply['content'][0]['text'])
        assert not reply.get('isError') and not value.get('error'), value
        print(label, name, round(records[-1]['seconds'], 3), flush=True)
        return value['result']
    try:
        rpc('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
            'clientInfo': {'name': 'release-region-profile', 'version': '1'}})
        proc.stdin.write(json.dumps({'jsonrpc': '2.0', 'method': 'notifications/initialized'}) + '\n'); proc.stdin.flush()
        for name in ['server_info', 'project_summary', 'schematic_list', 'component_list']: call(name)
        original_board = call('board_get')
        history = call('history')
        request = json.loads(Path(request_path).read_text())
        results, times = [], []
        for index in range(3):
            prepared = call('routing_region_prepare', request)
            times.append(records[-1]['seconds']); results.append(prepared)
            v = prepared['validation']
            assert not v['acceptable'] and v['netlist_preserved'] and v['new_error_count'] == 0
            assert v['before']['unrouted'] == 0 and v['after']['unrouted'] == 1
            assert [d['name'] for d in v['unrouted_regression_details']] == ['GND']
            assert prepared['routing']['unrouted'] == 0 and prepared['routing']['routed'] == 2
            preview = call('routing_preview', {'candidate': prepared['candidate'], 'limit': 200})
            assert preview['next_offset'] is None and not preview['stale']
            if index == 2:
                refused = rpc('tools/call', {'name': 'routing_apply', 'arguments': {
                    'candidate': prepared['candidate'], 'expected_revision': prepared['base_revision']}})
                assert refused.get('isError'), refused
                print(label, 'routing_apply refused', flush=True)
            call('routing_discard', {'candidate': prepared['candidate']})
        assert len({p['base_revision'] for p in results}) == 1
        assert call('history') == history
        assert call('board_get') == original_board
        drc = call('drc_run'); assert drc['errors'] == 3 and drc['warnings'] == 9
        call('project_close', {'discard_changes': True})
        proc.stdin.close(); assert proc.wait(timeout=10) == 0
    finally:
        if proc.poll() is None: proc.terminate(); proc.wait(timeout=10)
assert native_manifest() == native_before
log_bytes = log_path.read_bytes()
drc_times = [[float(x)/1000 for x in re.findall(rb'DRC succeeded after (\d+) ms', log_bytes[r['drc_log_start']:r['drc_log_end']])]
    for r in records if r['request'].get('params', {}).get('name') == 'routing_region_prepare']
summary = {'binary': binary, 'binary_sha256': hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
    'prepare_seconds': times, 'prepare_drc_seconds': drc_times, 'baseline_reused': [p.get('baseline_reused', False) for p in results],
    'validations': [p['validation'] for p in results], 'routing': [p['routing'] for p in results],
    'base_revisions': [p['base_revision'] for p in results], 'apply_refused': True, 'history_unchanged': True,
    'board_unchanged': True, 'native_unchanged': True, 'final_drc': drc}
(root / (label + '-summary.json')).write_text(json.dumps(summary, indent=2))
print(json.dumps({k:v for k,v in summary.items() if k not in ['validations', 'routing', 'final_drc']}, indent=2))
