"""Plot measured PCB copper in board coordinates (plane fills omitted)."""
import json,sys
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle,Circle

value=json.loads(Path(sys.argv[1]).read_text())
if 'response' in value:value=value['response']['result']['structuredContent']
b=value.get('result',value)
colors={'VCC':'#d17700','+5V':'#9b3497','GND':'#737e86','INT':'#11844c'}
fig,axes=plt.subplots(1,3,figsize=(17,7),sharex=True,sharey=True)
for ax,layer in zip(axes,['top_cu','bot_cu','in2_cu']):
 for t in b['traces']:
  if t['layer']!=layer:continue
  a,z=[t[e]['position'] for e in ['from','to']]
  if max(a['x'],z['x'])<107 or min(a['x'],z['x'])>117 or max(a['y'],z['y'])<-135 or min(a['y'],z['y'])>-126:continue
  ax.plot([a['x'],z['x']],[a['y'],z['y']],color=colors.get(t['net'],'#4669a3'),lw=t['width']*17,solid_capstyle='round',alpha=.85,zorder=2)
 for d in b['devices']:
  for p in d['pads']:
   x,y=p['position'].values()
   if not 107<=x<=117 or not -135<=y<=-126:continue
   if p['side'] not in ([('bottom' if layer=='bot_cu' else 'top'),'tht'] if layer!='in2_cu' else ['tht']):continue
   w,h=p['size']['width'],p['size']['height']
   ax.add_patch(Rectangle((x-w/2,y-h/2),w,h,angle=p['rotation'],rotation_point='center',fc=colors.get(p['net'],'#4669a3'),ec='white',lw=.5,zorder=3))
   if d['designator'] in ['U2','C29','C30','C4']:
    ax.text(x,y+.04,p['name'] if d['designator']=='U2' else p['pad'],fontsize=6.5,ha='center',va='bottom',rotation=0,zorder=5)
 for v in b['vias']:
  x,y=v['position'].values()
  if 107<=x<=117 and -135<=y<=-126:
   ax.add_patch(Circle((x,y),v['effective_size']/2,fc=colors.get(v['net'],'#4669a3'),ec='black',lw=.5,zorder=4))
   ax.add_patch(Circle((x,y),v['effective_drill']/2,fc='white',ec='black',lw=.3,zorder=5))
 ax.set(xlim=(107.2,116.9),ylim=(-134.7,-126.6),xlabel='x, mm',title=layer)
 ax.set_aspect('equal');ax.grid(alpha=.2);ax.set_axisbelow(True)
axes[0].set_ylabel('y, mm')
fig.suptitle(sys.argv[3] if len(sys.argv)>3 else 'U2 decoupling copper — board coordinates; pours omitted')
fig.tight_layout()
# Convert the trace widths in mm to points using each final axes scale.
for ax in axes:
 points_per_mm=ax.bbox.width / fig.dpi * 72 / (ax.get_xlim()[1]-ax.get_xlim()[0])
 for line in ax.lines:
  line.set_linewidth(line.get_linewidth() / 17 * points_per_mm)
fig.savefig(sys.argv[2],dpi=180);plt.close(fig)
