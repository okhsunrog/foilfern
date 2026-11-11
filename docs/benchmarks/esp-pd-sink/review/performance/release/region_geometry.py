"""Measure frozen regional patches without applying rejected candidates.

Fixture-specific replay of the three board mutation variants present here.
Coordinates are converted from native nm to mm. This does not rebuild planes,
measure plane copper, or replace the candidate's full validation.
"""
import hashlib
import json
import math
from pathlib import Path
import sys

records = json.loads(Path(sys.argv[1]).read_text())
request = json.loads(Path(sys.argv[2]).read_text())

def values(name):
    return [r['response']['result']['structuredContent']['result'] for r in records
        if r['request']['params'].get('name') == name]

board = values('board_get')[0]
previews = values('routing_preview')

def xy(p): return p['x'], p['y']
def nm_xy(p): return p['x']/1e6, p['y']/1e6

def measure(preview):
    positions = {}
    segments = {}
    net_names = dict(zip(preview['regional']['request']['nets'], request['route']['nets'], strict=True))
    for device in board['devices']:
        for pad in device['pads']:
            positions[('FootprintPad', device['component'], pad['uuid'])] = xy(pad['position'])
    def live_anchor(e):
        if 'pad_uuid' in e:
            label = e['pad'].split('.', 1)[0]
            d = next(d for d in board['devices'] if d['designator'] == label)
            return ('FootprintPad', d['component'], e['pad_uuid'])
        for field, kind in [('via', 'Via'), ('junction', 'Junction'), ('board_pad', 'Pad')]:
            if field in e:
                key = (kind, e[field]); positions[key] = xy(e['position']); return key
        raise ValueError(e)
    for t in board['traces']:
        s = segments.setdefault(t['segment'], {'net': t['net'], 'traces': {}, 'vias': {}})
        s['traces'][t['uuid']] = {'layer': t['layer'], 'width': t['width'],
            'a': live_anchor(t['from']), 'b': live_anchor(t['to'])}
    for v in board['vias']:
        positions[('Via', v['uuid'])] = xy(v['position'])
        s = segments.setdefault(v['segment'], {'net': v['net'], 'traces': {}, 'vias': {}})
        s['vias'][v['uuid']] = {'position': xy(v['position']), 'size': v['size'], 'drill': v['drill'],
            'start_layer': v['start_layer'], 'end_layer': v['end_layer']}
    def raw_anchor(e):
        kind, v = next(iter(e.items()))
        return (kind, v['device'], v['pad']) if kind == 'FootprintPad' else (kind, v)
    def stats():
        traces, vias, added_length = [], [], 0
        for s in segments.values():
            for t in s['traces'].values():
                a, b = positions[t['a']], positions[t['b']]
                added_length += math.dist(a, b)
                ends = sorted((json.dumps({'position': point, 'pad': anchor[1:] if anchor[0]=='FootprintPad' else None,
                    'kind': anchor[0]}, sort_keys=True) for anchor, point in [(t['a'], a), (t['b'], b)]))
                traces.append(json.dumps({'net': s['net'], 'layer': t['layer'], 'width': t['width'], 'ends': ends}, sort_keys=True))
            vias.extend(json.dumps({'net': s['net'], **v}, sort_keys=True) for v in s['vias'].values())
        return {'trace_count': len(traces), 'via_count': len(vias), 'trace_length_mm': added_length,
            'geometry_sha256': hashlib.sha256(json.dumps([sorted(traces), sorted(vias)]).encode()).hexdigest()}
    before = stats()
    for item in preview['mutations']:
        kind, data = next(iter(item['Board'].items()))
        if kind == 'RemoveNetSegment':
            del segments[data['segment']]
            continue
        if kind == 'AddNetSegment':
            value = data['segment']; key = value['uuid']
            assert not value['pads']
            segment = segments[key] = {'net': net_names[value['net']], 'traces': {}, 'vias': {}}
            elements = {k: list(value[k].values()) for k in ['junctions', 'traces', 'vias']}
        elif kind == 'AddNetSegmentElements':
            segment = segments[data['segment']['segment']]; elements = data['elements']
            assert not elements['pads']
        else: raise ValueError(f'Unsupported mutation: {kind}')
        for j in elements['junctions']: positions[('Junction', j['uuid'])] = nm_xy(j['position'])
        for v in elements['vias']:
            pos = nm_xy(v['position']); positions[('Via', v['uuid'])] = pos
            segment['vias'][v['uuid']] = {'position': pos,
                'size': None if v['size'] is None else v['size']/1e6,
                'drill': None if v['drill_diameter'] is None else v['drill_diameter']/1e6,
                'start_layer': v['start_layer'], 'end_layer': v['end_layer']}
        for t in elements['traces']:
            segment['traces'][t['uuid']] = {'layer': t['layer'], 'width': t['width']/1e6,
                'a': raw_anchor(t['p1']), 'b': raw_anchor(t['p2'])}
    return {'before': before, 'candidate': stats()}

print(json.dumps([measure(p) for p in previews], indent=2))
