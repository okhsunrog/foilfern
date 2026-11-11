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
colors={'VCC':'#d17700','GND':'#737e86','Net-(D4-A)':'#11844c'}
fig,axes=plt.subplots(1,2,figsize=(13,7),sharex=True,sharey=True)
for ax,layer in zip(axes,['bot_cu','top_cu']):
 for t in b['traces']:
  if t['layer']!=layer:continue
  a,z=[t[e]['position'] for e in ['from','to']]
  if max(a['x'],z['x'])<151 or min(a['x'],z['x'])>161 or max(a['y'],z['y'])<-115 or min(a['y'],z['y'])>-105:continue
  ax.plot([a['x'],z['x']],[a['y'],z['y']],color=colors.get(t['net'],'#4669a3'),lw=t['width']*17,solid_capstyle='round',alpha=.85,zorder=2)
 for d in b['devices']:
  for p in d['pads']:
   x,y=p['position'].values()
   if not 151<=x<=161 or not -115<=y<=-105:continue
   if p['side'] not in [('bottom' if layer=='bot_cu' else 'top'),'tht']:continue
   w,h=p['size']['width'],p['size']['height']
   ax.add_patch(Rectangle((x-w/2,y-h/2),w,h,angle=p['rotation'],rotation_point='center',fc=colors.get(p['net'],'#4669a3'),ec='white',lw=.5,zorder=3))
   if d['designator'] in ['R22','C27','D4'] or d['designator']=='OLED1':
    ax.text(x,y+.04,p['pad'],fontsize=6.5,ha='center',va='bottom',rotation=90 if d['designator']=='OLED1' else 0,zorder=5)
 for v in b['vias']:
  x,y=v['position'].values()
  if 151<=x<=161 and -115<=y<=-105:
   ax.add_patch(Circle((x,y),v['effective_size']/2,fc=colors.get(v['net'],'#4669a3'),ec='black',lw=.5,zorder=4))
   ax.add_patch(Circle((x,y),v['effective_drill']/2,fc='white',ec='black',lw=.3,zorder=5))
 ax.set(xlim=(151.5,160.5),ylim=(-114.7,-105.2),xlabel='x, mm',title=layer)
 ax.set_aspect('equal');ax.grid(alpha=.2);ax.set_axisbelow(True)
axes[0].set_ylabel('y, mm')
fig.suptitle(sys.argv[3] if len(sys.argv)>3 else 'OLED RESET copper — board coordinates; pours omitted')
fig.tight_layout()
# Convert the trace widths in mm to points using each final axes scale.
for ax in axes:
 points_per_mm=ax.bbox.width / fig.dpi * 72 / (ax.get_xlim()[1]-ax.get_xlim()[0])
 for line in ax.lines:
  line.set_linewidth(line.get_linewidth() / 17 * points_per_mm)
fig.savefig(sys.argv[2],dpi=180);plt.close(fig)
