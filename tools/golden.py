"""Computes golden values with the reference reader; the Rust tests assert the same values. Normalization follows
mc-structure's Structure::digest."""
import hashlib, json, sys
import nbt_le
EMPTY={'minecraft:air','minecraft:cave_air','minecraft:void_air','minecraft:structure_void'}
def state_str(name, states):
    if not states: return name
    parts=[]
    for k in sorted(states):
        t,v=states[k]
        parts.append(f'{k}={v}')
    return f"{name}[{','.join(parts)}]"
def bedrock(path):
    b=open(path,'rb').read(); _,root,_,_=nbt_le.read(b); r=root[1]
    size=[x[1] for x in r['size'][1]]; origin=[x[1] for x in r['structure_world_origin'][1]]
    st=r['structure'][1]; layers=st['block_indices'][1]
    def layer(t):
        k,v=t
        return v if k=='ints' else [x[1] for x in v]
    l0=layer(layers[0]); l1=layer(layers[1]) if len(layers)>1 else None
    pal=st['palette'][1]['default'][1]['block_palette'][1]
    states=[state_str(e[1]['name'][1], e[1]['states'][1]) for e in pal]
    names=[e[1]['name'][1] for e in pal]
    sx,sy,sz=size; cells=[]
    for i,p in enumerate(l0):
        if p<0 or names[p] in EMPTY: continue
        x=i//(sy*sz); y=(i//sz)%sy; z=i%sz
        s=states[p]
        if l1 is not None and l1[i]>=0 and names[l1[i]] not in EMPTY: s+='+'+states[l1[i]]
        cells.append(((x,y,z),s))
    cells.sort()
    h=hashlib.sha256(f'{sx}x{sy}x{sz}\n'.encode())
    for (x,y,z),s in cells: h.update(f'{x},{y},{z} {s}\n'.encode())
    mats={}
    for _,s in cells:
        n=s.split('[')[0].split('+')[0]; mats[n]=mats.get(n,0)+1
    mats=sorted(mats.items(), key=lambda kv:(-kv[1],kv[0]))
    bpd=st['palette'][1]['default'][1]['block_position_data'][1]
    return dict(format_version=r['format_version'][1], size=size, origin=origin, palette=len(pal), cells=len(cells),
                digest=h.hexdigest(), top=mats[:5], first=cells[0], block_entities=len(bpd), layers=len(layers))
for p in sys.argv[1:]:
    print(p.split('/')[-1], json.dumps(bedrock(p), ensure_ascii=False))
