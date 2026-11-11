"""Gate-clamp review measurements, including actual trace-layer changes.

Usage: measure_gate.py BOARD_JSON COMPONENT_LIST_JSON NATIVE_PROJECT_DIR [INTENT_JSON]
via_count retains the shared graph's count of via anchors on a path. A via used
as a same-layer junction is not a barrel traversal; layer_changes distinguishes it.
"""
import importlib.util
import json
from pathlib import Path
import sys

sys.dont_write_bytecode = True
shared = Path(__file__).with_name('measure.py')
if not shared.exists():
    shared = Path(__file__).parent.parent / 'measure.py'
spec = importlib.util.spec_from_file_location('review_measure', shared)
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


def load_result(path):
    value = json.loads(Path(path).read_text())
    if 'response' in value:
        value = value['response']['result']['structuredContent']
    return value.get('result', value)


if __name__ == '__main__':
    board_path, components_path, project_path, *intent_path = sys.argv[1:]
    intent = json.loads(Path(intent_path[0] if intent_path else Path(__file__).with_name('intent.json')).read_text())
    board = load_result(board_path)
    report = measure.review(board, load_result(components_path), Path(project_path), intent)
    traces = {trace['uuid']: trace for trace in board['traces']}
    for requirement in report['requirements']:
        measurement = requirement.get('measurement', {})
        if measurement.get('status') == 'measured':
            layers = [traces[uuid]['layer'] for uuid in measurement['traces']]
            measurement['layer_changes'] = sum(a != b for a, b in zip(layers, layers[1:]))
    print(json.dumps(report, indent=2))
