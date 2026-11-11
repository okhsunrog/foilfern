import json,sys
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.patches import Rectangle,Circle
v=json.load(open(sys.argv[1]));b=v.get('result',v)
colors={'ADC_VBUS':'#b44083','Net-(U2-VBUS)':'#d07c00','VBUS':'#c94733','GND':'#8d999d','D+':'#27977a','D-':'#265d9a'}
fig,axes=plt.subplots(1,3,figsize=(16,12),sharex=True,sharey=True)
for ax,layer in zip(axes,['top_cu','bot_cu','in2_cu']):
 for t in b['traces']:
  if t['layer']!=layer:continue
  a,z=[t[e]['position'] for e in ['from','to']]
  ax.plot([a['x'],z['x']],[a['y'],z['y']],color=colors.get(t['net'],'#b1bbcd'),lw=t['width']*12,solid_capstyle='round',zorder=2)
 for d in b['devices']:
  for q in d['pads']:
   x,y=q['position'].values()
   if not 103<=x<=119 or not -133<=y<=-111:continue
   if q['side'] not in ([('bottom' if layer=='bot_cu' else 'top'),'tht'] if layer!='in2_cu' else ['tht']):continue
   w,h=q['size']['width'],q['size']['height'];ax.add_patch(Rectangle((x-w/2,y-h/2),w,h,angle=q['rotation'],rotation_point='center',fc=colors.get(q['net'],'#506ca0'),ec='white',lw=.4,zorder=3))
   if d['designator'] in ['C4','R10','R11','SW5'] or (d['designator']=='U1' and q['name']=='8'):ax.text(x,y+.1,q['pad'],ha='center',fontsize=7,zorder=6)
 for q in b['vias']:
  x,y=q['position'].values()
  if 103<x<119 and -133<y<-111:
   ax.add_patch(Circle((x,y),q['effective_size']/2,fc=colors.get(q['net'],'#506ca0'),ec='black',lw=.4,zorder=4));ax.add_patch(Circle((x,y),q['effective_drill']/2,fc='white',ec='black',lw=.3,zorder=5))
 ax.set(xlim=(103.5,118.5),ylim=(-132.8,-112.8),title=layer,xlabel='x, mm');ax.set_aspect('equal');ax.grid(alpha=.2);ax.set_axisbelow(True)
axes[0].set_ylabel('y, mm');fig.suptitle(sys.argv[3]);fig.tight_layout()
for ax in axes:
 scale=ax.bbox.width/fig.dpi*72/(ax.get_xlim()[1]-ax.get_xlim()[0])
 for line in ax.lines:line.set_linewidth(line.get_linewidth()/12*scale)
fig.savefig(sys.argv[2],dpi=160)
