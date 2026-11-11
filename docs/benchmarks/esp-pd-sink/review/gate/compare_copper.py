"""Canonical primitives for the linear positive-polarity Gerber subset emitted here.

This is a read-only comparison helper, not a general Gerber parser. It rejects
unknown commands, arcs or format changes instead of claiming equivalence.
"""
from collections import Counter
import re

def canonical_contour(points):
 if len(points)>1 and points[0]==points[-1]:points=points[:-1]
 def rotate(q):
  smallest=min(q);return min(tuple(q[i:]+q[:i]) for i,v in enumerate(q) if v==smallest)
 return min(rotate(points),rotate(list(reversed(points))))

def primitives(path):
 macros={};apertures={};current=None;position=None;region=None;contour=[];out=Counter();format_seen=units_seen=False
 for line in path.read_text().splitlines():
  if line.startswith('G04'):continue
  if line=='%FSLAX66Y66*%':format_seen=True;continue
  if line=='%MOMM*%':units_seen=True;continue
  if line in ['G01*','G75*','M02*']:continue
  if line.startswith('%AM'):
   name,body=line[3:-1].split('*',1);macros[name]=body;continue
  match=re.fullmatch(r'%ADD(\d+)([^*]+)\*%',line)
  if match:
   id,shape=match.groups();apertures[int(id)]=('macro:'+macros[shape]) if shape in macros else shape;continue
  match=re.fullmatch(r'D(\d+)\*',line)
  if match:current=apertures[int(match[1])];continue
  if line=='G36*':assert region is None;region=[];contour=[];continue
  if line=='G37*':
   if contour:region.append(canonical_contour(contour))
   out[('region',tuple(sorted(region)))]+=1;region=None;contour=[];continue
  match=re.fullmatch(r'X(-?\d+)Y(-?\d+)D0([123])\*',line)
  if match:
   x,y,op=map(int,match.groups());next_position=(x,y)
   if region is not None:
    assert op in [1,2]
    if op==2:
     if contour:region.append(canonical_contour(contour))
     contour=[next_position]
    else:contour.append(next_position)
   elif op==3:out[('flash',current,next_position)]+=1
   elif op==1:
    assert position is not None;out[('stroke',current,tuple(sorted([position,next_position])))]+=1
   position=next_position;continue
  raise ValueError(f'Unsupported command: {line}')
 assert format_seen and units_seen and region is None
 return out

def drill_primitives(path):
 tools={};current=None;route=None;out=Counter();metric=absolute=False
 for line in path.read_text().splitlines():
  if line.startswith(';'):continue
  match=re.fullmatch(r'T(\d+)C([0-9.]+)',line)
  if match:tools[int(match[1])]=match[2];continue
  match=re.fullmatch(r'T(\d+)',line)
  if match:assert route is None;current=tools[int(match[1])] if int(match[1]) else None;continue
  if line=='G90':absolute=True;continue
  if line=='M71':metric=True;continue
  if line in ['M48','FMAT,2','METRIC,TZ','%','M30']:continue
  if line=='G05':
   if route is not None:out[('route',current,tuple(route))]+=1;route=None
   continue
  if re.fullmatch(r'G00X-?[0-9.]+Y-?[0-9.]+',line):assert route is None;route=[line];continue
  if route is not None:
   assert line in ['M15','M16'] or re.fullmatch(r'G01X-?[0-9.]+Y-?[0-9.]+',line),line
   route.append(line);continue
  if re.fullmatch(r'X-?[0-9.]+Y-?[0-9.]+',line):out[('hit',current,line)]+=1;continue
  raise ValueError(f'Unsupported drill command: {line}')
 assert absolute and metric and route is None
 return out

def plane_regions(path,net):
 """Compare emitted copper regions carrying the requested TO.N attribute."""
 current_net=None;region=None;contour=[];out=Counter()
 for line in path.read_text().splitlines():
  if line.startswith('G04 #@! TO.N,'):current_net=line[len('G04 #@! TO.N,'):-1]
  if line=='G36*':region=[];contour=[];continue
  if line=='G37*':
   if contour:region.append(canonical_contour(contour))
   if current_net==net:out[tuple(sorted(region))]+=1
   region=None;contour=[];continue
  if region is not None:
   match=re.fullmatch(r'X(-?\d+)Y(-?\d+)D0([12])\*',line);assert match,line
   x,y,op=map(int,match.groups())
   if op==2:
    if contour:region.append(canonical_contour(contour))
    contour=[(x,y)]
   else:contour.append((x,y))
 assert region is None
 return out
