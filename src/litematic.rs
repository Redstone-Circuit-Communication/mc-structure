//! Litematica 投影（`.litematic`）：gzip 压缩的 Java NBT，一个或多个「区域」，每个区域一张调色板加一个位压缩的 `BlockStates`。
//!
//! ```text
//! MinecraftDataVersion: Int           游戏的数据版本（和原版结构的 DataVersion 同义）
//! Version: Int, SubVersion?: Int      投影格式版本（见到的是 4 到 7）
//! Metadata: {…}                       只是参考：TotalBlocks、EnclosingSize 都不当真，以解码出来的为准
//! Regions: {
//!   <区域名>: {
//!     Position: {x, y, z}             区域在投影里的位置
//!     Size: {x, y, z}                 每个轴都可能是负数：负的是从 Position 往负方向延伸
//!     BlockStatePalette: List<{Name, Properties?}>      和原版结构的 palette 同一种写法
//!     BlockStates: LongArray          按调色板下标紧密打包（跨 long 的边界接着放）
//!     TileEntities: List<{x, y, z, …}>   坐标相对这个区域的最小角（也见过叫 BlockEntities 的，两个都认）
//!     Entities: List<{Pos, …}>        Pos 是相对区域的浮点坐标
//!   }
//! }
//! ```
//!
//! **区域的范围**：每个轴 `min = Position + (Size < 0 ? Size + 1 : 0)`，长度 `|Size|`。
//!
//! **`BlockStates` 的打包**：每个下标占 `bits = max(2, ceil(log2(调色板长度)))` 位；下标 `i` 从第 `i * bits` 位开始，
//! **跨 long 的边界接着放**（和 1.16 以后区块里「不跨 long、补齐」的写法不一样），第一个 long 的最低位是第 0 位；
//! 格子的下标是 `i = y * |Size.x| * |Size.z| + z * |Size.x| + x`，x、y、z 相对区域的最小角。
//! 长度至少是 `ceil(体积 * bits / 64)`（Litematica 写的正好是这么多；多出来的不管，有的工具会补零），下标不能超出调色板，否则是坏文件。
//!
//! **多个区域**合成一个 [`Structure`]：全部区域的范围合起来就是整个结构的范围（最小角移到 0,0,0），各区域按自己的偏移放进去；
//! 两个区域在同一格都有非空方块时，后面的区域覆盖前面的（Litematica 本身也是这样摆的）；空气不覆盖任何东西。
//! 解析成和原版结构**同一个** `Structure`：同一个结构存成 `.nbt` 或 `.litematic`，`digest()`、`block_count()`、`materials()` 都一样。

use std::collections::HashMap;

use crate::java::palette_from;
use crate::nbt::{Compound, Tag};
use crate::{Block, BlockEntity, BlockState, Edition, Error, Format, Limits, Structure};

/// 区域数的上限（真实的投影很少超过十几个）。
const MAX_REGIONS: usize = 64;

/// 根上有 `Regions` 复合标签就是投影。不拿 `MinecraftDataVersion` 当判据：老版本的投影没有它。
pub(crate) fn is_litematic(root: &Compound) -> bool {
    root.get("Regions").and_then(Tag::as_compound).is_some()
}

/// 一个区域的位置与范围：只读头部，还没有解码方块。
struct Region<'a> {
    name: &'a str,
    body: &'a Compound,
    /// 最小角（投影坐标系）
    min: [i64; 3],
    /// 每个轴的长度（`|Size|`）
    dims: [u64; 3],
}

impl Region<'_> {
    /// 格子数（读头部时已经核对过不会溢出）。
    fn volume(&self) -> u64 {
        let [x, y, z] = self.dims;
        x.saturating_mul(y).saturating_mul(z)
    }
}

/// 解码的结果：各区域合并在一起。
#[derive(Default)]
struct Decoded {
    /// 统一的调色板：各区域的调色板合并，相同的状态只留一份
    palette: Vec<BlockState>,
    index: HashMap<BlockState, u32>,
    blocks: Vec<Block>,
    block_entities: Vec<BlockEntity>,
    entities: Vec<Compound>,
}

impl Decoded {
    /// 状态在统一调色板里的下标，没有就加进去。
    fn intern(&mut self, st: BlockState) -> Result<u32, Error> {
        if let Some(i) = self.index.get(&st) {
            return Ok(*i);
        }
        let i = u32::try_from(self.palette.len()).map_err(|_| Error::TooLarge("调色板太大".into()))?;
        self.index.insert(st.clone(), i);
        self.palette.push(st);
        Ok(i)
    }
}

/// 读 `{x, y, z}` 三个整数。
fn xyz(c: &Compound, key: &str, path: &str) -> Result<[i64; 3], Error> {
    let v = c.get(key).and_then(Tag::as_compound).ok_or_else(|| Error::invalid(&format!("{path}.{key}"), "缺少"))?;
    let mut out = [0i64; 3];
    for (o, axis) in out.iter_mut().zip(["x", "y", "z"]) {
        let n = v.get(axis).and_then(Tag::as_i32).ok_or_else(|| Error::invalid(&format!("{path}.{key}.{axis}"), "缺少，或者不是整数"))?;
        *o = i64::from(n);
    }
    Ok(out)
}

/// 读一个区域的头部并核对它的范围。
fn read_region<'a>(name: &'a str, body: &'a Compound, limits: &Limits) -> Result<Region<'a>, Error> {
    let path = format!("Regions.{name}");
    let pos = xyz(body, "Position", &path)?;
    let size = xyz(body, "Size", &path)?;
    let mut min = [0i64; 3];
    let mut dims = [0u64; 3];
    for (((m, d), p), s) in min.iter_mut().zip(dims.iter_mut()).zip(pos).zip(size) {
        // 负的 Size：从 Position 往负方向延伸，最小角是 Position + Size + 1
        *m = if s < 0 { p + s + 1 } else { p };
        *d = s.unsigned_abs();
    }
    let max_axis = u64::from(limits.max_axis);
    let [dx, dy, dz] = dims;
    if dims.iter().any(|d| !(1..=max_axis).contains(d)) {
        return Err(Error::TooLarge(format!("区域「{name}」的尺寸 {dx}×{dy}×{dz} 不在 1..={max_axis} 之内")));
    }
    if dx.checked_mul(dy).and_then(|v| v.checked_mul(dz)).is_none_or(|v| v > limits.max_elements) {
        return Err(Error::TooLarge(format!("区域「{name}」的体积超过上限 {}", limits.max_elements)));
    }
    Ok(Region { name, body, min, dims })
}

/// 每个下标占的位数：`max(2, ceil(log2(调色板长度)))`。
fn bits_for(palette_len: usize) -> u32 {
    let n = palette_len.max(1) as u64;
    (64 - (n - 1).leading_zeros()).max(2)
}

/// 第 `i` 个下标：从第 `i * bits` 位开始，跨 long 的边界接着放，第一个 long 的最低位是第 0 位。读不到（数据不够长）返回 `None`。
fn unpack(longs: &[i64], bits: u32, i: u64, mask: u64) -> Option<u64> {
    let start = i.checked_mul(u64::from(bits))?;
    let word = usize::try_from(start >> 6).ok()?;
    let off = u32::try_from(start & 63).ok()?;
    let low = longs.get(word)?.cast_unsigned() >> off;
    let v = if off + bits <= 64 {
        low
    } else {
        // 跨了 long 的边界：低位在这个 long 的高处，高位在下一个 long 的低处
        low | (longs.get(word.checked_add(1)?)?.cast_unsigned() << (64 - off))
    };
    Some(v & mask)
}

/// 区域内的坐标 → 结构里的坐标（u32）。
fn coord(v: u64) -> Result<u32, Error> {
    u32::try_from(v).map_err(|_| Error::TooLarge("坐标超出范围".into()))
}

/// 实体的 `Pos`（相对区域的三个 Double）加上区域的偏移；没有 `Pos` 或写得不对就原样留着。
fn shift_entity(c: &Compound, shift: [f64; 3]) -> Compound {
    Compound(
        c.0.iter()
            .map(|(k, v)| match (k.as_str(), v) {
                ("Pos", Tag::List(p)) if p.len() == 3 => {
                    let moved = p.iter().zip(shift).map(|(t, o)| if let Tag::Double(d) = t { Tag::Double(d + o) } else { t.clone() }).collect();
                    (k.clone(), Tag::List(moved))
                }
                _ => (k.clone(), v.clone()),
            })
            .collect(),
    )
}

/// 解码一个区域：方块（空气不要）、方块实体、实体，都换成整个结构里的坐标。
fn decode_region(r: &Region<'_>, offset: [i64; 3], out: &mut Decoded) -> Result<(), Error> {
    let path = format!("Regions.{}", r.name);

    // 调色板：区域里的下标 → （统一调色板里的下标，是不是空气）
    let pal_path = format!("{path}.BlockStatePalette");
    let list = r.body.get("BlockStatePalette").and_then(Tag::as_list).ok_or_else(|| Error::invalid(&pal_path, "缺少"))?;
    let local = palette_from(list, &pal_path)?;
    if local.is_empty() {
        return Err(Error::invalid(&pal_path, "调色板是空的"));
    }
    let mut remap: Vec<(u32, bool)> = Vec::with_capacity(local.len());
    for st in local {
        let empty = st.is_empty();
        remap.push((out.intern(st)?, empty));
    }

    // BlockStates：长度至少是 ceil(体积 × 位数 / 64)，少了就是缺数据。多出来的不管（Litematica 写的正好是这么多，别的工具可能补零；
    // 站长以前的项目也是这样读的）。先核对再解码，不按文件里声称的尺寸直接分配
    let states_path = format!("{path}.BlockStates");
    let Some(Tag::LongArray(longs)) = r.body.get("BlockStates") else {
        return Err(Error::invalid(&states_path, "缺少，或者不是 LongArray"));
    };
    let bits = bits_for(remap.len());
    let [rx, ry, rz] = r.dims;
    let expected = r.volume().saturating_mul(u64::from(bits)).div_ceil(64);
    if (longs.len() as u64) < expected {
        let n = longs.len();
        return Err(Error::invalid(&states_path, format!("长度 {n} 不够：{rx}×{ry}×{rz} 个格子、每个 {bits} 位，至少要 {expected} 个 long")));
    }

    let [ox, oy, oz] = offset.map(|v| u64::try_from(v).unwrap_or(0));
    let mask = (1u64 << bits) - 1;
    for x in 0..rx {
        for y in 0..ry {
            for z in 0..rz {
                // 格子的下标：x 最快、然后 z、然后 y
                let i = (y * rz + z) * rx + x;
                let idx = unpack(longs, bits, i, mask).ok_or_else(|| Error::invalid(&states_path, format!("读不到第 {i} 个下标（数据不够长）")))?;
                let Some(&(state, empty)) = usize::try_from(idx).ok().and_then(|k| remap.get(k)) else {
                    let n = remap.len();
                    return Err(Error::invalid(&states_path, format!("第 {i} 个格子的调色板下标 {idx} 越界（调色板只有 {n} 项）")));
                };
                if !empty {
                    out.blocks.push(Block { pos: [coord(ox + x)?, coord(oy + y)?, coord(oz + z)?], state, extra: None });
                }
            }
        }
    }

    // 方块实体：x、y、z 相对这个区域的最小角。键名一般是 TileEntities，也见过 BlockEntities：两个都认，先看 BlockEntities（同站长以前的项目）
    let tiles = ["BlockEntities", "TileEntities"].into_iter().find_map(|key| r.body.get(key).and_then(Tag::as_list).map(|list| (key, list)));
    if let Some((key, list)) = tiles {
        for (k, t) in list.iter().enumerate() {
            let p = format!("{path}.{key}[{k}]");
            let c = t.as_compound().ok_or_else(|| Error::invalid(&p, "不是复合标签"))?;
            let at = |axis: &str, len: u64| -> Result<u64, Error> {
                c.get(axis)
                    .and_then(Tag::as_i32)
                    .and_then(|v| u64::try_from(v).ok())
                    .filter(|v| *v < len)
                    .ok_or_else(|| Error::invalid(&format!("{p}.{axis}"), "缺少，或者超出区域"))
            };
            let pos = [coord(ox + at("x", rx)?)?, coord(oy + at("y", ry)?)?, coord(oz + at("z", rz)?)?];
            // 位置已经在 pos 里，数据里区域内的 x、y、z 不再留
            let data = Compound(c.0.iter().filter(|e| !(matches!(e.0.as_str(), "x" | "y" | "z") && e.1.as_i32().is_some())).cloned().collect());
            out.block_entities.push(BlockEntity { pos, data });
        }
    }

    // 实体：Pos 是相对区域的浮点坐标
    if let Some(list) = r.body.get("Entities").and_then(Tag::as_list) {
        let shift = [ox as f64, oy as f64, oz as f64];
        for c in list.iter().filter_map(Tag::as_compound) {
            out.entities.push(shift_entity(c, shift));
        }
    }
    Ok(())
}

/// 把投影的根复合标签解析成 [`Structure`]。
pub(crate) fn from_root(root: &Compound, limits: &Limits) -> Result<Structure, Error> {
    let regions = root.get("Regions").and_then(Tag::as_compound).ok_or_else(|| Error::invalid("Regions", "缺少"))?;
    if regions.is_empty() {
        return Err(Error::invalid("Regions", "没有任何区域"));
    }
    if regions.len() > MAX_REGIONS {
        return Err(Error::TooLarge(format!("区域超过 {MAX_REGIONS} 个")));
    }

    // 第一遍：只读头部。核对每个区域和合起来的范围、体积，免得为一个超大的投影白白解码
    let mut headers = Vec::with_capacity(regions.len());
    let mut cells = 0u64;
    for (name, body) in regions.iter() {
        let body = body.as_compound().ok_or_else(|| Error::invalid(&format!("Regions.{name}"), "不是复合标签"))?;
        let r = read_region(name, body, limits)?;
        cells = cells.saturating_add(r.volume());
        if cells > limits.max_elements {
            return Err(Error::TooLarge(format!("各区域的体积加起来超过上限 {}", limits.max_elements)));
        }
        headers.push(r);
    }

    // 整个结构的范围：全部区域的范围合起来，最小角移到 (0, 0, 0)
    let mut lo = [i64::MAX; 3];
    let mut hi = [i64::MIN; 3];
    for r in &headers {
        for (((l, h), m), d) in lo.iter_mut().zip(hi.iter_mut()).zip(r.min).zip(r.dims) {
            *l = (*l).min(m);
            *h = (*h).max(m.saturating_add(i64::try_from(d).unwrap_or(i64::MAX)));
        }
    }
    let [lx, ly, lz] = lo;
    let [hx, hy, hz] = hi;
    let (sx, sy, sz) = (hx.saturating_sub(lx), hy.saturating_sub(ly), hz.saturating_sub(lz));
    let axis = |v: i64| u32::try_from(v).ok().filter(|v| (1..=limits.max_axis).contains(v));
    let (Some(ax), Some(ay), Some(az)) = (axis(sx), axis(sy), axis(sz)) else {
        return Err(Error::TooLarge(format!("投影的范围 {sx}×{sy}×{sz} 不在 1..={} 之内", limits.max_axis)));
    };
    if u64::from(ax).checked_mul(u64::from(ay)).and_then(|v| v.checked_mul(u64::from(az))).is_none_or(|v| v > limits.max_elements) {
        return Err(Error::TooLarge(format!("投影的体积 {ax}×{ay}×{az} 超过上限 {}", limits.max_elements)));
    }

    // 第二遍：解码。各区域按自己的偏移放进去
    let mut out = Decoded::default();
    for r in &headers {
        let [mx, my, mz] = r.min;
        decode_region(r, [mx - lx, my - ly, mz - lz], &mut out)?;
    }

    // 稳定排序：同一格的方块保持区域的先后顺序。两个区域在同一格都有非空方块时后面的覆盖前面的：
    // 倒过来去重（留每一格的第一个 = 最后写入的）再倒回来，不用另外分配。方块实体同理
    out.blocks.sort_by_key(|b| b.pos);
    out.blocks.reverse();
    out.blocks.dedup_by_key(|b| b.pos);
    out.blocks.reverse();
    out.block_entities.sort_by_key(|b| b.pos);
    out.block_entities.reverse();
    out.block_entities.dedup_by_key(|b| b.pos);
    out.block_entities.reverse();

    let format_version = root.get("MinecraftDataVersion").and_then(Tag::as_i32).unwrap_or(0);
    Ok(Structure {
        edition: Edition::Java,
        format: Format::Litematic,
        format_version,
        size: [ax, ay, az],
        origin: None,
        palette: out.palette,
        blocks: out.blocks,
        block_entities: out.block_entities,
        entities: out.entities,
    })
}
