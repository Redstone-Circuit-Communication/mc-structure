#!/usr/bin/env python3
"""Litematica 投影（.litematic）与原版结构（.nbt）的独立参考实现：写（造夹具）、读（算黄金值）。与 mc-structure 不共享代码。

  python3 litematic.py gen  <目录>      重新生成夹具（tests/fixtures 下的 door_*、multi_region*、palette*）
  python3 litematic.py read <文件...>   读 .litematic / .nbt，每个文件输出一行 JSON：格式、尺寸、方块数、摘要、材料前五……

规范化规则（摘要、空气）与 mc-structure 的 Structure::digest 相同（也见 golden.py）。
读投影时位打包用「把整个 long 数组当成一个大整数、按位切」的办法解出来——和 Rust 里「两个 long 之间移位拼接」的写法完全不同，
所以两边读出来一致才有意义。"""
import gzip
import hashlib
import json
import os
import struct
import sys

EMPTY = {"minecraft:air", "minecraft:cave_air", "minecraft:void_air", "minecraft:structure_void"}

# ───────────────────────── NBT（Java：大端；夹具里只有 BMP 字符，UTF-8 与修改版 UTF-8 一样）
# 一个标签是 (类型号, 值)；列表的值是 (元素类型号, [元素的裸值])；复合标签的值是 dict（保持写入顺序）


def tb(v): return (1, v)
def ti(v): return (3, v)
def tl(v): return (4, v)
def tf(v): return (5, v)
def td(v): return (6, v)
def tstr(v): return (8, v)
def tlist(et, items): return (9, (et, items))
def tcomp(d): return (10, d)
def tlongs(v): return (12, v)


def _w_str(b, s):
    e = s.encode("utf-8")
    b += struct.pack(">H", len(e)) + e


def _w_payload(b, t):
    tid, v = t
    if tid == 1: b += struct.pack(">b", v)
    elif tid == 2: b += struct.pack(">h", v)
    elif tid == 3: b += struct.pack(">i", v)
    elif tid == 4: b += struct.pack(">q", v)
    elif tid == 5: b += struct.pack(">f", v)
    elif tid == 6: b += struct.pack(">d", v)
    elif tid == 7: b += struct.pack(">i", len(v)) + bytes(x & 0xFF for x in v)
    elif tid == 8: _w_str(b, v)
    elif tid == 9:
        et, items = v
        b += struct.pack(">bi", et, len(items))
        for x in items:
            _w_payload(b, (et, x))
    elif tid == 10:
        for k, t2 in v.items():
            b.append(t2[0])
            _w_str(b, k)
            _w_payload(b, t2)
        b.append(0)
    elif tid == 11: b += struct.pack(">i", len(v)) + b"".join(struct.pack(">i", x) for x in v)
    elif tid == 12: b += struct.pack(">i", len(v)) + b"".join(struct.pack(">q", x) for x in v)
    else: raise ValueError(tid)


def write_nbt(root):
    b = bytearray([10])
    _w_str(b, "")
    _w_payload(b, root)
    return bytes(b)


class _R:
    def __init__(self, b):
        self.b, self.p = b, 0

    def u(self, fmt):
        n = struct.calcsize(fmt)
        v = struct.unpack_from(fmt, self.b, self.p)[0]
        self.p += n
        return v

    def string(self):
        n = self.u(">H")
        s = self.b[self.p:self.p + n].decode("utf-8")
        self.p += n
        return s

    def payload(self, tid):
        if tid == 1: return (1, self.u(">b"))
        if tid == 2: return (2, self.u(">h"))
        if tid == 3: return (3, self.u(">i"))
        if tid == 4: return (4, self.u(">q"))
        if tid == 5: return (5, self.u(">f"))
        if tid == 6: return (6, self.u(">d"))
        if tid == 7:
            n = self.u(">i"); v = list(struct.unpack_from(f">{n}b", self.b, self.p)); self.p += n
            return (7, v)
        if tid == 8: return (8, self.string())
        if tid == 9:
            et = self.u(">b"); n = self.u(">i")
            return (9, (et, [self.payload(et)[1] for _ in range(n)]))
        if tid == 10:
            d = {}
            while True:
                t = self.u(">b")
                if t == 0: return (10, d)
                k = self.string()
                d[k] = self.payload(t)
        if tid == 11:
            n = self.u(">i"); v = list(struct.unpack_from(f">{n}i", self.b, self.p)); self.p += 4 * n
            return (11, v)
        if tid == 12:
            n = self.u(">i"); v = list(struct.unpack_from(f">{n}q", self.b, self.p)); self.p += 8 * n
            return (12, v)
        raise ValueError(tid)


def read_nbt(raw):
    r = _R(raw)
    assert r.u(">b") == 10
    r.string()
    return r.payload(10)[1]


def load(path):
    raw = open(path, "rb").read()
    if raw[:2] == b"\x1f\x8b":
        raw = gzip.decompress(raw)
    return read_nbt(raw)


def gz(raw):
    return gzip.compress(raw, compresslevel=9, mtime=0)  # 固定 mtime：夹具每次生成都一样

# ───────────────────────── 方块状态与摘要（规则同 mc-structure）


def state_str(name, props):
    if not props:
        return name
    return f"{name}[{','.join(f'{k}={props[k]}' for k in sorted(props))}]"


def digest_of(size, cells):
    """cells: {(x,y,z): 规范写法}（只含非空格子）"""
    h = hashlib.sha256(f"{size[0]}x{size[1]}x{size[2]}\n".encode())
    for (x, y, z), s in sorted(cells.items()):
        h.update(f"{x},{y},{z} {s}\n".encode())
    return h.hexdigest()


def summary(fmt, version, size, cells, extra):
    mats = {}
    for s in cells.values():
        n = s.split("[")[0]
        mats[n] = mats.get(n, 0) + 1
    top = sorted(mats.items(), key=lambda kv: (-kv[1], kv[0]))[:5]
    out = dict(format=fmt, format_version=version, size=list(size), cells=len(cells), digest=digest_of(size, cells), top=top)
    out.update(extra)
    return out

# ───────────────────────── 读（参考实现）


def _props(entry):
    p = entry.get("Properties")
    return {k: v[1] for k, v in p[1].items()} if p else {}


def read_vanilla(root):
    size = [t[1] for t in [(0, v) for v in root["size"][1][1]]]
    pal = [(e["Name"][1], _props(e)) for e in root["palette"][1][1]]
    cells, bes = {}, []
    for b in root["blocks"][1][1]:
        pos = tuple(b["pos"][1][1])
        name, props = pal[b["state"][1]]
        if "nbt" in b:
            bes.append(pos)
        if name in EMPTY:
            continue
        cells[pos] = state_str(name, props)
    return summary("nbt", root["DataVersion"][1], size, cells, dict(block_entities=len(bes), entities=len(root["entities"][1][1]) if "entities" in root else 0))


def read_litematic(root):
    regions = root["Regions"][1]
    heads = []
    for name, rt in regions.items():
        rd = rt[1]
        pos = [rd["Position"][1][k][1] for k in "xyz"]
        size = [rd["Size"][1][k][1] for k in "xyz"]
        mn = [p + (s + 1 if s < 0 else 0) for p, s in zip(pos, size)]
        heads.append((name, rd, mn, [abs(s) for s in size]))
    lo = [min(h[2][k] for h in heads) for k in range(3)]
    hi = [max(h[2][k] + h[3][k] for h in heads) for k in range(3)]
    size = [hi[k] - lo[k] for k in range(3)]
    cells, bes, ents = {}, [], 0
    for name, rd, mn, dims in heads:
        pal = [(e["Name"][1], _props(e)) for e in rd["BlockStatePalette"][1][1]]
        bits = max(2, (len(pal) - 1).bit_length())
        vol = dims[0] * dims[1] * dims[2]
        longs = rd["BlockStates"][1]
        assert len(longs) >= (vol * bits + 63) // 64, f"{name}: BlockStates 不够长"
        big = int.from_bytes(b"".join(struct.pack("<q", v) for v in longs), "little")  # 整个数组当一个大整数，第一个 long 的最低位是第 0 位
        mask = (1 << bits) - 1
        for i in range(vol):
            idx = (big >> (i * bits)) & mask
            x, z, y = i % dims[0], (i // dims[0]) % dims[2], i // (dims[0] * dims[2])
            n, props = pal[idx]
            if n in EMPTY:
                continue
            cells[(mn[0] + x - lo[0], mn[1] + y - lo[1], mn[2] + z - lo[2])] = state_str(n, props)  # 后面的区域覆盖前面的
        for te in rd.get("BlockEntities", rd.get("TileEntities", (9, (10, []))))[1][1]:
            bes.append((mn[0] + te["x"][1] - lo[0], mn[1] + te["y"][1] - lo[1], mn[2] + te["z"][1] - lo[2]))
        ents += len(rd.get("Entities", (9, (10, [])))[1][1])
    bes.sort()
    return summary("litematic", root["MinecraftDataVersion"][1], size, cells, dict(block_entities=len(bes), block_entity_pos=[list(p) for p in bes], entities=ents, regions=len(heads), version=root["Version"][1]))


def read_any(path):
    root = load(path)
    return read_litematic(root) if "Regions" in root else read_vanilla(root)

# ───────────────────────── 写（造夹具）


def pack_bits(indices, bits):
    """紧密打包：下标 i 从第 i*bits 位开始，跨 long 的边界接着放；返回有符号 long 的列表。"""
    big = 0
    for i, v in enumerate(indices):
        assert 0 <= v < (1 << bits)
        big |= v << (i * bits)
    out = []
    for k in range((len(indices) * bits + 63) // 64):
        x = (big >> (64 * k)) & ((1 << 64) - 1)
        out.append(x - (1 << 64) if x >= 1 << 63 else x)
    return out


def xyz(x, y, z):
    return tcomp({"x": ti(x), "y": ti(y), "z": ti(z)})


def palette_tag(palette):
    items = []
    for name, props in palette:
        c = {"Name": tstr(name)}
        if props:
            c["Properties"] = tcomp({k: tstr(v) for k, v in props.items()})
        items.append(c)
    return tlist(10, items)


def region(position, size, palette, cells, tiles=(), ents=()):
    """cells: {(x,y,z): 调色板下标}，坐标相对区域的最小角；没写的格子是 0 号。tiles: [(x,y,z,{其余键})]；ents: [{...}]"""
    dx, dy, dz = abs(size[0]), abs(size[1]), abs(size[2])
    bits = max(2, (len(palette) - 1).bit_length())
    idx = [0] * (dx * dy * dz)
    for (x, y, z), v in cells.items():
        assert 0 <= x < dx and 0 <= y < dy and 0 <= z < dz, (x, y, z)
        idx[y * dx * dz + z * dx + x] = v
    te = []
    for x, y, z, rest in tiles:
        d = {"x": ti(x), "y": ti(y), "z": ti(z)}
        d.update(rest)
        te.append(d)
    return tcomp({
        "Position": xyz(*position), "Size": xyz(*size),
        "BlockStatePalette": palette_tag(palette), "BlockStates": tlongs(pack_bits(idx, bits)),
        "TileEntities": tlist(10, te), "Entities": tlist(10, list(ents)),
        "PendingBlockTicks": tlist(10, []), "PendingFluidTicks": tlist(10, []),
    })


def litematic(version, regions, data_version=3953, lies=False):
    meta = {"Name": tstr("fixture"), "Author": tstr("rsw"), "Description": tstr(""), "RegionCount": ti(len(regions)),
            "TotalBlocks": ti(1 if lies else 0), "TotalVolume": ti(1 if lies else 0), "EnclosingSize": xyz(1, 1, 1) if lies else xyz(0, 0, 0),
            "TimeCreated": tl(0), "TimeModified": tl(0)}  # lies：元数据故意写错，解析器不该当真
    root = {"MinecraftDataVersion": ti(data_version), "Version": ti(version)}
    if version >= 7:
        root["SubVersion"] = ti(1)
    root["Metadata"] = tcomp(meta)
    root["Regions"] = tcomp(dict(regions))
    return gz(write_nbt(tcomp(root)))


def vanilla(size, palette, cells, data_version=3953, block_nbt=None):
    """原版结构：把盒子里的每一格都写进 blocks（空气也写）；cells: {(x,y,z): 调色板下标}；block_nbt: {(x,y,z): 复合标签的裸值}"""
    block_nbt = block_nbt or {}
    blocks = []
    for x in range(size[0]):
        for y in range(size[1]):
            for z in range(size[2]):
                d = {"pos": tlist(3, [x, y, z]), "state": ti(cells.get((x, y, z), 0))}
                if (x, y, z) in block_nbt:
                    d["nbt"] = tcomp(block_nbt[(x, y, z)])
                blocks.append(d)
    root = {"DataVersion": ti(data_version), "size": tlist(3, list(size)), "palette": palette_tag(palette), "blocks": tlist(10, blocks), "entities": tlist(10, [])}
    return gz(write_nbt(tcomp(root)))

# ───────────────────────── 夹具的内容

AIR = ("minecraft:air", {})
STONE = ("minecraft:stone", {})
QUARTZ = ("minecraft:quartz_block", {})
PISTON_C = ("minecraft:sticky_piston", {"facing": "up", "extended": "false"})
PISTON_O = ("minecraft:sticky_piston", {"facing": "up", "extended": "true"})
HEAD = ("minecraft:piston_head", {"facing": "up", "short": "false", "type": "sticky"})
WIRE0 = ("minecraft:redstone_wire", {"east": "side", "north": "none", "power": "0", "south": "none", "west": "side"})
WIRE15 = ("minecraft:redstone_wire", {"east": "side", "north": "none", "power": "15", "south": "none", "west": "side"})
OBS = ("minecraft:observer", {"facing": "north", "powered": "false"})
OBS_ON = ("minecraft:observer", {"facing": "north", "powered": "true"})

DOOR_SIZE = (5, 4, 3)


def door_cells(opened):
    """{(x,y,z): 状态}，坐标 0 起"""
    c = {}
    for x in range(5):
        for z in range(3):
            c[(x, 0, z)] = STONE
    for x in (0, 4):
        for y in (1, 2):
            c[(x, y, 0)] = QUARTZ
    for x in (1, 2, 3):
        c[(x, 1, 1)] = PISTON_O if opened else PISTON_C
        if opened:
            c[(x, 2, 1)] = HEAD
    c[(2, 1, 2)] = WIRE15 if opened else WIRE0
    c[(2, 2, 2)] = OBS_ON if opened else OBS
    return c


def skey(s):
    return (s[0], tuple(sorted(s[1].items())))


def uniq(states):
    """去重（状态里有 dict，不能直接放进 set），按名字与属性排序"""
    return sorted({skey(s): s for s in states}.values(), key=skey)


def indexed(cells, palette):
    return {p: palette.index(s) for p, s in cells.items()}


def door_files(opened):
    cells = door_cells(opened)
    # 原版结构：调色板按首次出现的顺序；litematic：空气在 0 号，其余按名字倒序——两边的调色板顺序不一样
    vp = [AIR] + uniq(cells.values())
    lp = [AIR] + uniq(cells.values())[::-1]
    nbt = vanilla(DOOR_SIZE, vp, indexed(cells, vp))
    if opened:  # 开启态：第 7 版、负尺寸（Position 在最大角，区域往负方向延伸）
        lit = litematic(7, [("door", region((4, 3, 2), (-5, -4, -3), lp, indexed(cells, lp)))])
    else:       # 关闭态：第 6 版、正尺寸、Position 不在原点
        lit = litematic(6, [("door", region((3, 5, 7), (5, 4, 3), lp, indexed(cells, lp)))])
    return nbt, lit


SIGN = ("minecraft:oak_sign", {"rotation": "0", "waterlogged": "false"})
CHEST = ("minecraft:chest", {"facing": "north", "type": "single", "waterlogged": "false"})
DIRT = ("minecraft:dirt", {})
OBSIDIAN = ("minecraft:obsidian", {})


def multi_region():
    """三个区域：base（正尺寸）、wall（和 base 在 x=3 重叠、后面的覆盖前面的）、neg（负尺寸、带方块实体与实体）。各自的调色板不一样。"""
    base_p = [AIR, STONE, DIRT]
    base = {(x, 0, z): 1 for x in range(4) for z in range(3)}
    base[(1, 1, 1)] = 2
    base[(2, 1, 1)] = 2
    wall_p = [AIR, QUARTZ, SIGN]
    wall = {(0, y, z): 1 for y in range(2) for z in range(3)}
    wall[(1, 1, 1)] = 2
    neg_p = [AIR, OBSIDIAN, CHEST]
    neg = {(x, y, z): 1 for x in range(2) for y in range(3) for z in range(2)}
    del neg[(1, 2, 1)]  # 一个空气
    neg[(0, 0, 0)] = 2
    sign_te = {"id": tstr("minecraft:sign"), "front_text": tcomp({"has_glowing_text": tb(0)}), "is_waxed": tb(0)}
    chest_te = {"id": tstr("minecraft:chest"), "Items": tlist(10, [])}
    stand = {"id": tstr("minecraft:armor_stand"), "Pos": tlist(6, [0.5, 0.0, 0.5]), "Motion": tlist(6, [0.0, 0.0, 0.0]), "Rotation": tlist(5, [0.0, 0.0])}
    regions = [
        ("base", region((0, 0, 0), (4, 3, 3), base_p, base)),
        ("wall", region((3, 0, 0), (3, 4, 3), wall_p, wall, tiles=[(1, 1, 1, sign_te)])),
        ("neg", region((7, 5, 3), (-2, -3, -2), neg_p, neg, tiles=[(0, 0, 0, chest_te)], ents=[stand])),
    ]
    lit = litematic(6, regions, lies=True)
    # 同样的结构压平成一个原版结构（独立地叠一遍：后面的覆盖前面的；方块实体也带上）
    flat = {}
    for (px, py, pz), (cells, pal) in [((0, 0, 0), (base, base_p)), ((3, 0, 0), (wall, wall_p)), ((6, 3, 2), (neg, neg_p))]:
        for (x, y, z), v in cells.items():
            if pal[v] not in (AIR,):
                flat[(px + x, py + y, pz + z)] = pal[v]
    fp = [AIR] + uniq(flat.values())
    nbt = vanilla((8, 6, 4), fp, indexed(flat, fp), block_nbt={(4, 1, 1): sign_te, (6, 3, 2): chest_te})
    return lit, nbt


def many(palette_size, size, step, add):
    """调色板很大的区域：下标按 (i*step+add) % palette_size 循环；名字用中继器的 64 种状态加空气，凑到想要的大小"""
    states = [AIR]
    for delay in "1234":
        for facing in ("north", "south", "east", "west"):
            for locked in ("true", "false"):
                for powered in ("true", "false"):
                    states.append(("minecraft:repeater", {"delay": delay, "facing": facing, "locked": locked, "powered": powered}))
    palette = states[:palette_size]
    n = size[0] * size[1] * size[2]
    cells = {}
    for i in range(n):
        v = (i * step + add) % palette_size
        x, z, y = i % size[0], (i // size[0]) % size[2], i // (size[0] * size[2])
        cells[(x, y, z)] = v
    return litematic(6, [("many", region((0, 0, 0), size, palette, cells))])


def gen(out):
    os.makedirs(out, exist_ok=True)
    files = {}
    for name, opened in (("close", False), ("open", True)):
        nbt, lit = door_files(opened)
        files[f"door_{name}.nbt"], files[f"door_{name}.litematic"] = nbt, lit
    lit, flat = multi_region()
    files["multi_region.litematic"], files["multi_region_flat.nbt"] = lit, flat
    files["palette17.litematic"] = many(17, (7, 5, 3), 7, 3)    # 5 位：105 个下标 = 525 位，跨 long 的边界很多次
    files["palette64.litematic"] = many(64, (5, 4, 4), 5, 1)    # 正好 2 的 6 次方：6 位，64 不是 6 的倍数，也跨边界
    for name, data in files.items():
        with open(os.path.join(out, name), "wb") as f:
            f.write(data)
    return sorted(files)


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "gen":
        for n in gen(sys.argv[2]):
            print("写了", n)
    elif len(sys.argv) >= 3 and sys.argv[1] == "read":
        for p in sys.argv[2:]:
            print(os.path.basename(p), json.dumps(read_any(p), ensure_ascii=False))
    else:
        print(__doc__)
        sys.exit(2)
