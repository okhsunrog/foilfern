"""Validate read-only MCP path measurements against saved ESP-PD-Sink pairs.

Usage: replay.py RELEASE_BINARY NATIVE_SOURCE REVIEW_INTENT_JSON OUTPUT_DIR
The external review prototype beside intent.json is an independent comparator.
The source is copied. No mutations, saves, DRC or plane rebuilds are requested.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

sys.dont_write_bytecode = True
binary, source, intent_path, output = sys.argv[1:]
root = Path(output).resolve(); root.mkdir(parents=True, exist_ok=True)
folder = root / 'project'
shutil.copytree(source, folder, ignore=shutil.ignore_patterns('output', '*.lock', '*.user.lp'))
def manifest():
    return {str(p.relative_to(folder)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(folder.rglob('*')) if p.is_file() and not p.name.endswith(('.lock', '.user.lp'))}
native_before = manifest()
(root / 'native-manifest.json').write_text(json.dumps(native_before, indent=2))
log = (root / 'stderr.log').open('w')
proc = subprocess.Popen([binary, '--project', str(folder / 'ESP-PD-Sink.lpp')], stdin=subprocess.PIPE,
    stdout=subprocess.PIPE, stderr=log, text=True, bufsize=1, env=dict(os.environ, RUST_LOG='warn'))
records = []; sequence = 0

def rpc(method, params):
    global sequence
    sequence += 1
    request = {'jsonrpc':'2.0', 'id': sequence, 'method': method, 'params': params}
    start = time.perf_counter(); proc.stdin.write(json.dumps(request)+'\n'); proc.stdin.flush()
    while True:
        line = proc.stdout.readline()
        if not line: raise RuntimeError('Server exited')
        reply = json.loads(line)
        if reply.get('id') == sequence: break
    records.append({'request': request, 'response': reply, 'seconds':time.perf_counter()-start})
    (root / 'records.json').write_text(json.dumps(records, indent=2))
    assert 'error' not in reply, reply
    return reply['result']

def call(name, args=None, error=None):
    reply = rpc('tools/call', {'name': name, 'arguments':args or {}})
    value = reply.get('structuredContent') or json.loads(reply['content'][0]['text'])
    if error:
        assert reply.get('isError') and value['error']['kind'] == error, reply
        return value
    assert not reply.get('isError') and not value.get('error'), value
    return value['result']

try:
    rpc('initialize', {'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'path-api-replay','version':'1'}})
    proc.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n'); proc.stdin.flush()
    advertised = rpc('tools/list', {})
    assert any(t['name']=='trace_path_measure' for t in advertised['tools'])
    for name in ['server_info','project_summary','schematic_list','component_list']: call(name)
    board = call('board_get'); initial_history = call('history')
    spec = importlib.util.spec_from_file_location('review_prototype', Path(intent_path).with_name('measure.py'))
    prototype = importlib.util.module_from_spec(spec); spec.loader.exec_module(prototype)
    pairs = [r for r in json.loads(Path(intent_path).read_text())['requirements'] if r['kind']=='trace_path']
    comparisons = []; revision = None
    for pair in pairs:
        args = {'board':board['uuid'], 'from':pair['from'], 'to':pair['to']}
        if revision is not None: args['expected_revision'] = revision
        measured = call('trace_path_measure', args)
        elapsed = records[-1]['seconds']; revision = measured['base_revision']
        reference = prototype.measure(board, pair['from'], pair['to'])
        assert measured['board']==board['uuid'] and not measured['plane_copper_measured']
        if reference['status']=='measured':
            assert measured['status']=='measured', measured
            assert abs(measured['length_mm']-reference['length_mm']) < 1e-6
            assert measured['via_count']==reference['via_count'] and measured['layers']==reference['layers']
        else:
            assert measured['status']=='not_measured' and measured['length_mm'] is None
        comparisons.append({'from':pair['from'],'to':pair['to'],'prototype':reference,'mcp':measured,'seconds':elapsed})
        print(pair['from'], pair['to'], measured['status'], measured['length_mm'], flush=True)
    first = comparisons[0]['mcp']
    by_id = call('trace_path_measure', {'board':first['board'],'expected_revision':revision,
        'from':{'component':first['from']['component'],'pad':first['from']['pad']},
        'to':{'component':first['to']['component'],'pad':first['to']['pad']}})
    assert by_id == first
    call('trace_path_measure', {'from':'OLED1.27','to':'C22.1','expected_revision':revision+1}, error='stale_revision')
    call('trace_path_measure', {'from':'OLED1.27','to':'C21.1'}, error='invalid_argument')
    call('trace_path_measure', {'from':'OLED1.999','to':'C21.1'}, error='not_found')
    assert call('history')==initial_history
    assert call('board_get')==board
    call('project_close', {'discard_changes':True})
    proc.stdin.close(); assert proc.wait(timeout=10)==0
finally:
    if proc.poll() is None: proc.terminate(); proc.wait(timeout=10)
    log.close()
assert manifest()==native_before
summary = {'binary':binary,'binary_sha256':hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
    'profile':'release','comparisons':comparisons,'uuid_query_identical':True,'stale_revision_refused':True,
    'different_nets_refused':True,'missing_pad_refused':True,'board_unchanged':True,'history_unchanged':True,
    'native_unchanged':True,'revision':revision,'board':board['uuid']}
(root / 'summary.json').write_text(json.dumps(summary, indent=2))
print('Validated',len(comparisons),'pairs; native files, board and history unchanged.')
