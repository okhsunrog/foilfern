"""Run one source-free CLI task and a separately measured feedback turn."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

root = Path(sys.argv[1]).resolve()
model = sys.argv[2]
case = root / 'case'
user_home = Path.home()
args = ['taskset', '-c', '0,1', 'nice', '-n', '10', '/nix/store/m50dfhnbbqpvj79i444kcdd2zsfwr4hk-bubblewrap-0.13.1/bin/bwrap', '--ro-bind', '/', '/']
for hidden in ['code', 'tmp_zfs', 'pcb', 'LibrePCB-Workspace', '.claude', '.codex', '.agents']:
    path = user_home / hidden
    if path.exists():
        args += ['--tmpfs', str(path)]
args += ['--bind', str(root), str(root), '--bind', str(root/'codex-state'), str(user_home/'.codex'), '--tmpfs', '/tmp', '--unshare-pid', '--proc', '/proc', '--dev', '/dev', '--chdir', str(case), '--']
cli = ['codex', 'exec', '--ignore-user-config', '--ignore-rules', '--skip-git-repo-check', '--dangerously-bypass-approvals-and-sandbox', '-m', model, '-c', 'model_reasoning_effort="medium"', '-c', 'project_doc_max_bytes=0', '-c', 'project_root_markers=[]', '-c', 'web_search="disabled"', '--disable', 'apps', '--disable', 'memories', '--disable', 'multi_agent', '--disable', 'multi_agent_v2', '--disable', 'skill_search', '--json']
env = dict(os.environ)
for key in ['CODEX_HOME', 'CODEX_THREAD_ID', 'CODEX_INTERNAL_ORIGINATOR_OVERRIDE', 'VIRTUAL_ENV']:
    env.pop(key, None)
env.update(TMPDIR=str(root/'tmp'), UV_CACHE_DIR=str(root/'uv-cache'), CARGO_BUILD_JOBS='2', RAYON_NUM_THREADS='2', TOKIO_WORKER_THREADS='2')
feedback = 'Give detailed feedback about this task and the tools. Identify unclear descriptions, errors, workarounds, layout/wiring limitations, visual-review problems and unnecessary calls. Explain what should improve beyond MCP itself. Distinguish observed behavior from hypotheses. Limit feedback to 300 words. Do not edit the project in this feedback turn.'
metrics = {'model': model, 'reasoning_effort': 'medium', 'client': subprocess.check_output(['codex','--version'],text=True).strip(), 'source_access': False, 'cpu_affinity': [0,1], 'turns': []}
session = None
for phase, prompt in [('task', (case/'prompt.txt').read_text()), ('feedback', feedback)]:
    command = args + cli
    if phase == 'feedback':
        command += ['resume', session]
    command += ['-o', str(case/(phase+'-answer.md')), '-']
    start = time.monotonic()
    with (case/(phase+'-events.jsonl')).open('w') as output, (case/(phase+'-stderr.log')).open('w') as error:
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=output, stderr=error, env=env, start_new_session=True, text=True)
        try:
            process.communicate(prompt, timeout=int(os.environ.get('FOILFERN_TRIAL_DEADLINE_S','1800')) if phase=='task' else 120)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=10)
    events = []
    for line in (case/(phase+'-events.jsonl')).read_text().splitlines():
        try:
            events.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    sessions = [event['thread_id'] for event in events if event.get('type') == 'thread.started']
    if sessions:
        session = sessions[-1]
    completed = [event for event in events if event.get('type') == 'turn.completed']
    metrics['turns'].append({'phase': phase, 'wall_s': time.monotonic()-start, 'exit_code': process.returncode, 'usage': [event.get('usage') for event in completed], 'session_id':session})
    (case/'metrics.json').write_text(json.dumps(metrics, indent=2)+'\n')
    print(json.dumps(metrics['turns'][-1]), flush=True)
    if process.returncode or not completed or not session:
        raise SystemExit('Task failed, inspect retained logs')
