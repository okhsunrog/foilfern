"""Resolve manual review intent and measure trace centerlines from MCP evidence.

Usage: measure.py BOARD_JSON COMPONENT_LIST_JSON NATIVE_PROJECT_DIR [INTENT_JSON]
Both JSON inputs accept an MCP envelope or its result. Does not infer plane paths,
barrel length, loop inductance or electrical limits from net names.
"""
import heapq
import itertools
import json
import math
from pathlib import Path
import re
import sys


def result(path):
    value = json.loads(Path(path).read_text())
    return value.get('result', value)


def measure(board, first, last):
    pads = {p['uuid']: p for d in board['devices'] for p in d['pads']}
    starts = [p for p in pads.values() if p['pad'] == first]
    goals = [p for p in pads.values() if p['pad'] == last]
    if not starts or not goals:
        return {'status': 'not_checked', 'reason': 'Pad address did not resolve.'}
    pairs = [(a, b) for a in starts for b in goals if a['net'] == b['net']]
    if not pairs:
        return {'status': 'not_checked', 'reason': 'Pads have no common net.'}
    def xy(point):
        return point['x'], point['y']
    def key(endpoint):
        return endpoint.get('pad_uuid') or endpoint.get('via') or endpoint.get('junction') or endpoint.get('board_pad')
    graph = {}
    for trace in board['traces']:
        a, b = [key(trace[e]) for e in ['from', 'to']]
        length = math.dist(*(xy(trace[e]['position']) for e in ['from', 'to']))
        for u, v in [(a, b), (b, a)]:
            graph.setdefault((trace['net'], u), []).append((v, length, trace))
    choices = []
    for a, b in pairs:
        counter = itertools.count()
        queue = [(0.0, next(counter), a['uuid'], [])]
        visited = set()
        while queue:
            distance, _, node, route = heapq.heappop(queue)
            if node in visited:
                continue
            visited.add(node)
            if node == b['uuid']:
                choices.append((distance, math.dist(xy(a['position']), xy(b['position'])), route))
                break
            for other, length, trace in graph.get((a['net'], node), []):
                if other not in visited:
                    heapq.heappush(queue, (distance + length, next(counter), other, route + [trace]))
    if not choices:
        return {'status': 'not_checked', 'direct_mm': min(math.dist(xy(a['position']), xy(b['position'])) for a, b in pairs),
                'reason': 'No trace-only path. May connect through rebuilt copper planes; inspect plane coverage separately.'}
    length, direct, route = min(choices, key=lambda v: v[0])
    vias = {e['via'] for t in route for e in [t['from'], t['to']] if 'via' in e}
    return {'status': 'measured', 'direct_mm': direct, 'length_mm': length,
            'layers': sorted({t['layer'] for t in route}), 'via_count': len(vias),
            'widths_mm': sorted({t['width'] for t in route}), 'traces': [t['uuid'] for t in route]}


def review(board, components, project, intent):
    devices = {d['designator']: d for d in board['devices']}
    instances = {c['designator']: c for c in components['components']}
    def component(name):
        return {'component_uuid': instances[name]['uuid'], 'designator': name}
    def pin(address):
        name, number = address.rsplit('.', 1)
        device = devices[name]
        matches = [p for p in device['pads'] if p['name'] == number or p['signal'] == number]
        if not matches:
            raise ValueError(f'Unresolved pin: {address}')
        signals = {p['signal'] for p in matches}
        if len(signals) != 1:
            raise ValueError(f'Ambiguous signal: {address}')
        signal_name = next(iter(signals))
        library = project / 'library/cmp' / instances[name]['library_component']['uuid'] / 'component.lp'
        signal_ids = dict((label, uuid) for uuid, label in re.findall(r'\(signal ([\w-]+) \(name "([^"]+)"\)', library.read_text()))
        return {**component(name), 'signal_uuid': signal_ids[signal_name],
                'signal_name': signal_name, 'address': address,
                'footprint_pad_uuids': [p['uuid'] for p in matches]}
    groups = [{**g, 'components': [component(c) for c in g['components']],
               'pins': [pin(p) for p in g['pins']]} for g in intent['groups']]
    checks = []
    for requirement in intent['requirements']:
        check = dict(requirement)
        if requirement['kind'] == 'trace_path':
            check['from_ref'] = pin(requirement['from'])
            check['to_ref'] = pin(requirement['to'])
            check['measurement'] = measure(board, requirement['from'], requirement['to'])
            measurement = check['measurement']
            if measurement['status'] == 'measured':
                limits = []
                if 'max_length_mm' in requirement:
                    limits.append(measurement['length_mm'] <= requirement['max_length_mm'])
                if 'max_vias' in requirement:
                    limits.append(measurement['via_count'] <= requirement['max_vias'])
                check['status'] = ('pass' if all(limits) else 'missed_target') if limits else 'measured'
            else:
                check['status'] = 'not_checked'
        elif requirement['kind'] == 'fixed_placement':
            device = devices[requirement['component']]
            check['component_ref'] = component(requirement['component'])
            check['measurement'] = {key: device[key] for key in ['position', 'rotation', 'side']}
            check['status'] = 'measured'
        checks.append(check)
    return {'format': intent['format'], 'board_uuid': board['uuid'], 'groups': groups, 'requirements': checks}


if __name__ == '__main__':
    board_path, components_path, project_path, *intent_path = sys.argv[1:]
    intent = json.loads(Path(intent_path[0] if intent_path else Path(__file__).with_name('intent.json')).read_text())
    print(json.dumps(review(result(board_path), result(components_path), Path(project_path), intent), indent=2))
