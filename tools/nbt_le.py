"""小端 NBT 参考读取器（只用于生成黄金样本，不进产品）。"""
import struct
def read(b):
    pos=[0]
    def take(n):
        s=b[pos[0]:pos[0]+n]
        if len(s)!=n: raise ValueError(f'truncated at {pos[0]}')
        pos[0]+=n; return s
    def u8(): return take(1)[0]
    def i8(): return struct.unpack('<b',take(1))[0]
    def i16(): return struct.unpack('<h',take(2))[0]
    def i32(): return struct.unpack('<i',take(4))[0]
    def i64(): return struct.unpack('<q',take(8))[0]
    def f32(): return struct.unpack('<f',take(4))[0]
    def f64(): return struct.unpack('<d',take(8))[0]
    def s(): n=struct.unpack('<H',take(2))[0]; return take(n).decode('utf-8')
    def payload(t):
        if t==1: return ('byte',i8())
        if t==2: return ('short',i16())
        if t==3: return ('int',i32())
        if t==4: return ('long',i64())
        if t==5: return ('float',f32())
        if t==6: return ('double',f64())
        if t==7: n=i32(); return ('bytes',list(take(n)))
        if t==8: return ('string',s())
        if t==9:
            et=u8(); n=i32(); return ('list',[payload(et) for _ in range(n)])
        if t==10:
            d={}
            while True:
                tt=u8()
                if tt==0: return ('compound',d)
                k=s(); d[k]=payload(tt)
        if t==11: n=i32(); return ('ints',[i32() for _ in range(n)])
        if t==12: n=i32(); return ('longs',[i64() for _ in range(n)])
        raise ValueError(f'tag {t} at {pos[0]}')
    t=u8(); name=s(); v=payload(t)
    return name, v, pos[0], len(b)
def plain(v):
    k,x=v
    if k=='compound': return {a:plain(b) for a,b in x.items()}
    if k=='list': return [plain(i) for i in x]
    return x
