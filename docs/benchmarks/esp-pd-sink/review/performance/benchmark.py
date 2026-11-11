"""Cold/repeated prepare and non-empty checked apply on one immutable board.

Usage: benchmark.py BINARY LABEL PROJECT_DIR CANDIDATE_JSON. Both binaries use identical opt-level flags.
No native project edits: each process opens its own temporary copy.
"""
import json,os,subprocess,sys,time,shutil,hashlib,re
from pathlib import Path
root=Path(__file__).parent
binary,label,source,candidate_path=sys.argv[1:]
folder=root/('project-'+label)
shutil.copytree(source,folder,ignore=shutil.ignore_patterns('output','*.lock','*.user.lp'),dirs_exist_ok=True)
log=open(root/(label+'.stderr'),'w')
proc=subprocess.Popen([binary,'--project',str(folder/'ESP-PD-Sink.lpp')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True,bufsize=1,env=dict(os.environ,RUST_LOG='librepcb_core::project::board::drc=debug'))
records=[];sequence=0

def rpc(method,params):
 global sequence
 sequence+=1;request={'jsonrpc':'2.0','id':sequence,'method':method,'params':params};start=time.perf_counter()
 proc.stdin.write(json.dumps(request)+'\n');proc.stdin.flush()
 while True:
  line=proc.stdout.readline()
  if not line:raise RuntimeError('Server exited')
  reply=json.loads(line)
  if reply.get('id')==sequence:break
 records.append({'request':request,'response':reply,'seconds':time.perf_counter()-start})
 (root/(label+'-records.json')).write_text(json.dumps(records,indent=2))
 return reply

def call(name,args=None):
 reply=rpc('tools/call',{'name':name,'arguments':args or {}});result=reply['result'];value=result.get('structuredContent') or json.loads(result['content'][0]['text']);assert not value.get('error'),value
 print(label,name,round(records[-1]['seconds'],3),value.get('result',{}).get('baseline_reused'),flush=True)
 return value

rpc('initialize',{'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'baseline-reuse-benchmark','version':'1'}})
proc.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n');proc.stdin.flush()
for name in ['server_info','project_summary','history','schematic_list','component_list','board_get']:call(name)
request=json.loads(Path(candidate_path).read_text())
checks=[];times=[];reuse=[]
for _ in range(3):
 prepared=call('routing_copper_prepare',request);times.append(records[-1]['seconds']);r=prepared['result'];checks.append(r['validation']);reuse.append(r.get('baseline_reused',False))
 assert r['validation']['acceptable'] and r['validation']['after']['unrouted']==0 and r['validation']['new_error_count']==0 and r['validation']['netlist_preserved'];assert r['mutation_count']>0
 preview=call('routing_preview',{'candidate':r['candidate'],'limit':200})['result'];assert preview['next_offset'] is None and not preview['stale']
last=prepared['result'];applied=call('routing_apply',{'candidate':last['candidate'],'expected_revision':last['base_revision']});apply_seconds=records[-1]['seconds']
assert applied['result']['applied'] and applied['result']['validation']['acceptable']
final=call('drc_run')['result'];assert final['errors']==3 and final['warnings']==9
board=call('board_get')['result']
# Normalize only netsegment/generated junction identities, preserving geometry/net/width/layer and pad/via identities.
def anchor(e):return e.get('pad_uuid') or e.get('via') or tuple(e['position'].values())
traces=sorted((t['net'],t['layer'],t['width'],tuple(sorted((repr(anchor(t['from'])),repr(anchor(t['to'])))))) for t in board['traces'])
# Vias themselves have netsegment IDs which may differ after merge; keep canonical geometric proof separately.
via_geometry=sorted(repr({k:v for k,v in q.items() if k!='segment'}) for q in board['vias'])
geometry_digest=hashlib.sha256(repr((traces,via_geometry)).encode()).hexdigest()
peak=[line for line in Path(f'/proc/{proc.pid}/status').read_text().splitlines() if line.startswith('VmHWM:')][0]
call('project_close',{'discard_changes':True});proc.stdin.close();proc.wait(timeout=10);log.close()
summary={'label':label,'prepare_seconds':times,'baseline_reused':reuse,'apply_seconds':apply_seconds,'validations':checks,'applied_validation':applied['result']['validation'],'final_drc':final,'geometry_sha256':geometry_digest,'peak_process':peak,'drc_core_seconds':[float(x)/1000 for x in re.findall(r'DRC succeeded after (\d+) ms', (root/(label+'.stderr')).read_text())]}
(root/(label+'-summary.json')).write_text(json.dumps(summary,indent=2));print(json.dumps({k:v for k,v in summary.items() if k not in ['validations','applied_validation','final_drc']},indent=2))
