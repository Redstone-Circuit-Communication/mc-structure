//! Litematica schematic (`.litematic`): gzip-compressed Java NBT with one or more regions, each with its own palette and a
//! bit-packed `BlockStates`.
//!
//! ```text
//! MinecraftDataVersion: Int           game data version (same meaning as a vanilla structure's DataVersion)
//! Version: Int, SubVersion?: Int      schematic format version (4 to 7 seen in practice)
//! Metadata: {…}                       informational only: TotalBlocks and EnclosingSize are ignored in favour of the decoded data
//! Regions: {
//!   <region name>: {
//!     Position: {x, y, z}             position of the region inside the schematic
//!     Size: {x, y, z}                 any axis may be negative, meaning the region extends from Position in the negative direction
//!     BlockStatePalette: List<{Name, Properties?}>      same layout as a vanilla structure's palette
//!     BlockStates: LongArray          palette indices packed tightly (spanning long boundaries)
//!     TileEntities: List<{x, y, z, …}>   positions relative to the region's minimum corner (also seen as BlockEntities; both are read)
//!     Entities: List<{Pos, …}>        Pos is a floating-point position relative to the region
//!   }
//! }
//! ```
//!
//! **Region bounds**: on each axis `min = Position + (Size < 0 ? Size + 1 : 0)` and the length is `|Size|`.
//!
//! **`BlockStates` packing**: each index takes `bits = max(2, ceil(log2(palette length)))` bits; index `i` starts at bit `i * bits`
//! and **continues across long boundaries** (unlike chunk storage since 1.16, which pads instead); bit 0 is the lowest bit of the
//! first long.
//! The index of a cell is `i = y * |Size.x| * |Size.z| + z * |Size.x| + x`, with x, y, z relative to the region's minimum corner.
//! The array holds at least `ceil(volume * bits / 64)` longs (exactly that many when written by Litematica; extra longs, which some
//! tools pad with, are ignored), and every index must be inside the palette, otherwise the file is invalid.
//!
//! **Several regions** become one [`Structure`]: the union of all region bounds is the bounds of the structure (minimum corner moved to
//! 0,0,0), and each region is placed at its own offset.
//! When two regions both have a non-air block in the same cell, the later region wins (as Litematica places them); air never
//! overwrites anything.
//! The result is **the same** `Structure` as for a vanilla structure: one structure saved as `.nbt` or as `.litematic` has the same
//! `digest()`, `block_count()` and `materials()`.

use std::collections::HashMap;

use crate::java::palette_from;
use crate::nbt::{Compound, Tag};
use crate::{Block, BlockEntity, BlockState, Edition, Error, Format, Limits, Structure};

/// Maximum number of regions (real schematics rarely have more than a dozen or so).
const MAX_REGIONS: usize = 64;

/// A root with a `Regions` compound is a schematic. `MinecraftDataVersion` is not used for this: old schematics do not have it.
pub(crate) fn is_litematic(root: &Compound) -> bool {
    root.get("Regions").and_then(Tag::as_compound).is_some()
}

/// Position and bounds of a region, read from its header before any blocks are decoded.
struct Region<'a> {
    name: &'a str,
    body: &'a Compound,
    /// Minimum corner (schematic coordinates)
    min: [i64; 3],
    /// Length of each axis (`|Size|`)
    dims: [u64; 3],
}

impl Region<'_> {
    /// Number of cells (checked for overflow when the header was read).
    fn volume(&self) -> u64 {
        let [x, y, z] = self.dims;
        x.saturating_mul(y).saturating_mul(z)
    }
}

/// Decoding result, with all regions merged.
#[derive(Default)]
struct Decoded {
    /// Merged palette: the palettes of all regions, each distinct state once
    palette: Vec<BlockState>,
    index: HashMap<BlockState, u32>,
    blocks: Vec<Block>,
    block_entities: Vec<BlockEntity>,
    entities: Vec<Compound>,
}

impl Decoded {
    /// Index of a state in the merged palette, adding it if absent.
    fn intern(&mut self, st: BlockState) -> Result<u32, Error> {
        if let Some(i) = self.index.get(&st) {
            return Ok(*i);
        }
        let i = u32::try_from(self.palette.len()).map_err(|_| Error::TooLarge("palette too large".into()))?;
        self.index.insert(st.clone(), i);
        self.palette.push(st);
        Ok(i)
    }
}

/// Reads the three integers of `{x, y, z}`.
fn xyz(c: &Compound, key: &str, path: &str) -> Result<[i64; 3], Error> {
    let v = c.get(key).and_then(Tag::as_compound).ok_or_else(|| Error::invalid(&format!("{path}.{key}"), "missing"))?;
    let mut out = [0i64; 3];
    for (o, axis) in out.iter_mut().zip(["x", "y", "z"]) {
        let n = v.get(axis).and_then(Tag::as_i32).ok_or_else(|| Error::invalid(&format!("{path}.{key}.{axis}"), "missing or not an integer"))?;
        *o = i64::from(n);
    }
    Ok(out)
}

/// Reads a region header and checks its bounds.
fn read_region<'a>(name: &'a str, body: &'a Compound, limits: &Limits) -> Result<Region<'a>, Error> {
    let path = format!("Regions.{name}");
    let pos = xyz(body, "Position", &path)?;
    let size = xyz(body, "Size", &path)?;
    let mut min = [0i64; 3];
    let mut dims = [0u64; 3];
    for (((m, d), p), s) in min.iter_mut().zip(dims.iter_mut()).zip(pos).zip(size) {
        // A negative Size extends from Position in the negative direction; the minimum corner is Position + Size + 1
        *m = if s < 0 { p + s + 1 } else { p };
        *d = s.unsigned_abs();
    }
    let max_axis = u64::from(limits.max_axis);
    let [dx, dy, dz] = dims;
    if dims.iter().any(|d| !(1..=max_axis).contains(d)) {
        return Err(Error::TooLarge(format!("region \"{name}\" has size {dx}×{dy}×{dz}, outside 1..={max_axis}")));
    }
    if dx.checked_mul(dy).and_then(|v| v.checked_mul(dz)).is_none_or(|v| v > limits.max_elements) {
        return Err(Error::TooLarge(format!("region \"{name}\" exceeds the volume limit of {}", limits.max_elements)));
    }
    Ok(Region { name, body, min, dims })
}

/// Bits per index: `max(2, ceil(log2(palette length)))`.
fn bits_for(palette_len: usize) -> u32 {
    let n = palette_len.max(1) as u64;
    (64 - (n - 1).leading_zeros()).max(2)
}

/// Index number `i`: starts at bit `i * bits`, continues across long boundaries, and bit 0 is the lowest bit of the first long.
/// Returns `None` when the data is too short.
fn unpack(longs: &[i64], bits: u32, i: u64, mask: u64) -> Option<u64> {
    let start = i.checked_mul(u64::from(bits))?;
    let word = usize::try_from(start >> 6).ok()?;
    let off = u32::try_from(start & 63).ok()?;
    let low = longs.get(word)?.cast_unsigned() >> off;
    let v = if off + bits <= 64 {
        low
    } else {
        // Spans a long boundary: the low bits are at the top of this long, the high bits at the bottom of the next one
        low | (longs.get(word.checked_add(1)?)?.cast_unsigned() << (64 - off))
    };
    Some(v & mask)
}

/// Position inside a region → position inside the structure (u32).
fn coord(v: u64) -> Result<u32, Error> {
    u32::try_from(v).map_err(|_| Error::TooLarge("position out of range".into()))
}

/// Adds the region offset to an entity's `Pos` (three Doubles relative to the region); a missing or malformed `Pos` is left as
/// it is.
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

/// Decodes one region: blocks (without air), block entities and entities, all moved to structure coordinates.
fn decode_region(r: &Region<'_>, offset: [i64; 3], out: &mut Decoded) -> Result<(), Error> {
    let path = format!("Regions.{}", r.name);

    // Palette: index in the region → (index in the merged palette, whether it is air)
    let pal_path = format!("{path}.BlockStatePalette");
    let list = r.body.get("BlockStatePalette").and_then(Tag::as_list).ok_or_else(|| Error::invalid(&pal_path, "missing"))?;
    let local = palette_from(list, &pal_path)?;
    if local.is_empty() {
        return Err(Error::invalid(&pal_path, "palette is empty"));
    }
    let mut remap: Vec<(u32, bool)> = Vec::with_capacity(local.len());
    for st in local {
        let empty = st.is_empty();
        remap.push((out.intern(st)?, empty));
    }

    // BlockStates: at least ceil(volume × bits / 64) longs, fewer means missing data. Extra longs are ignored (Litematica writes
    // exactly that many, other tools may pad with zeros). The length is checked before decoding, so nothing is allocated from
    // sizes the file merely claims.
    let states_path = format!("{path}.BlockStates");
    let Some(Tag::LongArray(longs)) = r.body.get("BlockStates") else {
        return Err(Error::invalid(&states_path, "missing or not a LongArray"));
    };
    let bits = bits_for(remap.len());
    let [rx, ry, rz] = r.dims;
    let expected = r.volume().saturating_mul(u64::from(bits)).div_ceil(64);
    if (longs.len() as u64) < expected {
        let n = longs.len();
        return Err(Error::invalid(&states_path, format!("length {n} is too short: {rx}×{ry}×{rz} cells of {bits} bits need at least {expected} longs")));
    }

    let [ox, oy, oz] = offset.map(|v| u64::try_from(v).unwrap_or(0));
    let mask = (1u64 << bits) - 1;
    for x in 0..rx {
        for y in 0..ry {
            for z in 0..rz {
                // Cell index: x varies fastest, then z, then y
                let i = (y * rz + z) * rx + x;
                let idx = unpack(longs, bits, i, mask).ok_or_else(|| Error::invalid(&states_path, format!("cannot read index {i} (data too short)")))?;
                let Some(&(state, empty)) = usize::try_from(idx).ok().and_then(|k| remap.get(k)) else {
                    let n = remap.len();
                    return Err(Error::invalid(&states_path, format!("cell {i} has palette index {idx} out of range (the palette has {n} entries)")));
                };
                if !empty {
                    out.blocks.push(Block { pos: [coord(ox + x)?, coord(oy + y)?, coord(oz + z)?], state, extra: None });
                }
            }
        }
    }

    // Block entities: x, y, z are relative to the region's minimum corner. The key is usually TileEntities, sometimes
    // BlockEntities; both are read, BlockEntities first.
    let tiles = ["BlockEntities", "TileEntities"].into_iter().find_map(|key| r.body.get(key).and_then(Tag::as_list).map(|list| (key, list)));
    if let Some((key, list)) = tiles {
        for (k, t) in list.iter().enumerate() {
            let p = format!("{path}.{key}[{k}]");
            let c = t.as_compound().ok_or_else(|| Error::invalid(&p, "not a compound tag"))?;
            let at = |axis: &str, len: u64| -> Result<u64, Error> {
                c.get(axis)
                    .and_then(Tag::as_i32)
                    .and_then(|v| u64::try_from(v).ok())
                    .filter(|v| *v < len)
                    .ok_or_else(|| Error::invalid(&format!("{p}.{axis}"), "missing or outside the region"))
            };
            let pos = [coord(ox + at("x", rx)?)?, coord(oy + at("y", ry)?)?, coord(oz + at("z", rz)?)?];
            // The position now lives in `pos`, so the region-relative x, y, z are dropped from the data
            let data = Compound(c.0.iter().filter(|e| !(matches!(e.0.as_str(), "x" | "y" | "z") && e.1.as_i32().is_some())).cloned().collect());
            out.block_entities.push(BlockEntity { pos, data });
        }
    }

    // Entities: Pos is a floating-point position relative to the region
    if let Some(list) = r.body.get("Entities").and_then(Tag::as_list) {
        let shift = [ox as f64, oy as f64, oz as f64];
        for c in list.iter().filter_map(Tag::as_compound) {
            out.entities.push(shift_entity(c, shift));
        }
    }
    Ok(())
}

/// Parses the root compound of a schematic into a [`Structure`].
pub(crate) fn from_root(root: &Compound, limits: &Limits) -> Result<Structure, Error> {
    let regions = root.get("Regions").and_then(Tag::as_compound).ok_or_else(|| Error::invalid("Regions", "missing"))?;
    if regions.is_empty() {
        return Err(Error::invalid("Regions", "no regions"));
    }
    if regions.len() > MAX_REGIONS {
        return Err(Error::TooLarge(format!("more than {MAX_REGIONS} regions")));
    }

    // First pass: headers only. Checks the bounds and volume of each region and of their union, so an oversized schematic is
    // rejected before any decoding
    let mut headers = Vec::with_capacity(regions.len());
    let mut cells = 0u64;
    for (name, body) in regions.iter() {
        let body = body.as_compound().ok_or_else(|| Error::invalid(&format!("Regions.{name}"), "not a compound tag"))?;
        let r = read_region(name, body, limits)?;
        cells = cells.saturating_add(r.volume());
        if cells > limits.max_elements {
            return Err(Error::TooLarge(format!("total volume of all regions exceeds the limit of {}", limits.max_elements)));
        }
        headers.push(r);
    }

    // Bounds of the whole structure: the union of all region bounds, with the minimum corner moved to (0, 0, 0)
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
        return Err(Error::TooLarge(format!("schematic size {sx}×{sy}×{sz} is outside 1..={}", limits.max_axis)));
    };
    if u64::from(ax).checked_mul(u64::from(ay)).and_then(|v| v.checked_mul(u64::from(az))).is_none_or(|v| v > limits.max_elements) {
        return Err(Error::TooLarge(format!("schematic volume {ax}×{ay}×{az} exceeds the limit of {}", limits.max_elements)));
    }

    // Second pass: decode, placing each region at its own offset
    let mut out = Decoded::default();
    for r in &headers {
        let [mx, my, mz] = r.min;
        decode_region(r, [mx - lx, my - ly, mz - lz], &mut out)?;
    }

    // Stable sort: blocks in the same cell keep the order of their regions. When two regions both have a non-air block in the
    // same cell the later one wins: reverse, deduplicate (keeping the first of each cell, i.e. the last written) and reverse
    // back, without another allocation. Block entities are handled the same way
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
