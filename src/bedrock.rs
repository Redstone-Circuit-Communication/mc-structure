//! Bedrock `.mcstructure`: uncompressed little-endian NBT.
//!
//! ```text
//! format_version: Int                       1 or 2 (both occur in practice)
//! size: List<Int>[3]
//! structure_world_origin: List<Int>[3]      world position at the time of export
//! structure:
//!   block_indices: List<…>[1 or 2]          one integer sequence per layer, length = x·y·z; -1 means an empty cell
//!                                           a List<Int> in format_version 1, an IntArray in 2 (observed)
//!                                           layer 0 holds the main blocks, layer 1 the waterlogging layer
//!   entities: List<Compound>
//!   palette.default:
//!     block_palette: List<{name, states, version}>
//!     block_position_data: { "<linear index>": { block_entity_data: {…} } }
//! ```
//!
//! Linear index layout: z varies fastest, then y, then x: `i = x·(sy·sz) + y·sz + z`.
//! (Check: in the fixture, the block entity of cell `"167"` records the world position (-12,115,193) and the structure origin
//! is (-12,104,180); 167 unfolds to exactly (0,11,13).)

use crate::nbt::{self, Compound, Endian, Tag};
use crate::{Block, BlockEntity, BlockState, Edition, Error, Format, Limits, StateValue, Structure};

fn ints3(c: &Compound, key: &str) -> Result<Option<[i32; 3]>, Error> {
    let Some(t) = c.get(key) else { return Ok(None) };
    let v = t.as_ints().ok_or_else(|| Error::invalid(key, format!("expected three integers, found {}", t.type_name())))?;
    <[i32; 3]>::try_from(v.as_slice()).map(Some).map_err(|_| Error::invalid(key, format!("expected three integers, found {}", v.len())))
}

pub(crate) fn size_of(c: &Compound, key: &str, limits: &Limits) -> Result<[u32; 3], Error> {
    let s = ints3(c, key)?.ok_or_else(|| Error::invalid(key, "missing"))?;
    let mut out = [0u32; 3];
    for (o, v) in out.iter_mut().zip(s) {
        *o = u32::try_from(v)
            .ok()
            .filter(|v| (1..=limits.max_axis).contains(v))
            .ok_or_else(|| Error::TooLarge(format!("structure size {}×{}×{} is outside 1..={}", s[0], s[1], s[2], limits.max_axis)))?;
    }
    let volume = u64::from(out[0]) * u64::from(out[1]) * u64::from(out[2]);
    if volume > limits.max_elements {
        return Err(Error::TooLarge(format!("structure volume {volume} exceeds the limit of {}", limits.max_elements)));
    }
    Ok(out)
}

fn state_value(path: &str, t: &Tag) -> Result<StateValue, Error> {
    match t {
        Tag::Byte(v) => Ok(StateValue::Byte(*v)),
        Tag::Short(v) => Ok(StateValue::Int(i32::from(*v))),
        Tag::Int(v) => Ok(StateValue::Int(*v)),
        Tag::String(s) => Ok(StateValue::String(s.clone())),
        other => Err(Error::invalid(path, format!("unsupported block state value of type {}", other.type_name()))),
    }
}

fn palette(st: &Compound) -> Result<Vec<BlockState>, Error> {
    const P: &str = "structure.palette.default.block_palette";
    let list = st
        .get("palette")
        .and_then(Tag::as_compound)
        .and_then(|p| p.get("default"))
        .and_then(Tag::as_compound)
        .and_then(|d| d.get("block_palette"))
        .and_then(Tag::as_list)
        .ok_or_else(|| Error::invalid(P, "missing palette"))?;
    let mut out = Vec::with_capacity(list.len());
    for (i, e) in list.iter().enumerate() {
        let path = format!("{P}[{i}]");
        let e = e.as_compound().ok_or_else(|| Error::invalid(&path, "not a compound tag"))?;
        let name = e.get("name").and_then(Tag::as_str).ok_or_else(|| Error::invalid(&path, "missing name"))?;
        let mut states = Vec::new();
        if let Some(s) = e.get("states").and_then(Tag::as_compound) {
            for (k, v) in s.iter() {
                states.push((k.to_owned(), state_value(&format!("{path}.states.{k}"), v)?));
            }
        }
        out.push(BlockState::new(name, states));
    }
    Ok(out)
}

pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Structure, Error> {
    let (_, root) = nbt::read(bytes, Endian::Little, limits)?;
    let format_version = root.get("format_version").and_then(Tag::as_i32).ok_or_else(|| Error::invalid("format_version", "missing or not an integer"))?;
    let size = size_of(&root, "size", limits)?;
    let origin = ints3(&root, "structure_world_origin")?;
    let st = root.get("structure").and_then(Tag::as_compound).ok_or_else(|| Error::invalid("structure", "missing"))?;
    let palette = palette(st)?;

    let [sx, sy, sz] = size;
    let volume = (sx as usize) * (sy as usize) * (sz as usize);
    let layers_tag = st.get("block_indices").and_then(Tag::as_list).ok_or_else(|| Error::invalid("structure.block_indices", "missing"))?;
    if layers_tag.is_empty() || layers_tag.len() > 2 {
        return Err(Error::invalid("structure.block_indices", format!("expected 1 or 2 layers, found {}", layers_tag.len())));
    }
    let mut layers = Vec::with_capacity(layers_tag.len());
    for (i, l) in layers_tag.iter().enumerate() {
        let path = format!("structure.block_indices[{i}]");
        let v = l.as_ints().ok_or_else(|| Error::invalid(&path, format!("expected a sequence of integers, found {}", l.type_name())))?;
        if v.len() != volume {
            return Err(Error::invalid(&path, format!("length {} does not match the volume {volume}", v.len())));
        }
        layers.push(v);
    }
    let check = |path: &str, idx: i32| -> Result<Option<u32>, Error> {
        if idx < 0 {
            return Ok(None);
        }
        let i = usize::try_from(idx).unwrap_or(usize::MAX);
        match palette.get(i) {
            None => Err(Error::invalid(path, format!("palette index {idx} out of range (the palette has {} entries)", palette.len()))),
            Some(s) if s.is_empty() => Ok(None),
            Some(_) => Ok(Some(idx.unsigned_abs())),
        }
    };
    let mut blocks = Vec::new();
    let main = layers.first().map_or(&[][..], Vec::as_slice);
    let water = layers.get(1);
    let plane = (sy as usize) * (sz as usize);
    for (i, &idx) in main.iter().enumerate() {
        let Some(state) = check("structure.block_indices[0]", idx)? else { continue };
        let extra = match water.and_then(|w| w.get(i)) {
            Some(&e) => check("structure.block_indices[1]", e)?,
            None => None,
        };
        blocks.push(Block { pos: pos_of(i, plane, sz as usize), state, extra });
    }

    let mut block_entities = Vec::new();
    if let Some(bpd) = st
        .get("palette")
        .and_then(Tag::as_compound)
        .and_then(|p| p.get("default"))
        .and_then(Tag::as_compound)
        .and_then(|d| d.get("block_position_data"))
        .and_then(Tag::as_compound)
    {
        for (k, v) in bpd.iter() {
            let path = format!("structure.palette.default.block_position_data.{k}");
            let i: usize = k.parse().map_err(|_| Error::invalid(&path, "key should be a linear index"))?;
            if i >= volume {
                return Err(Error::invalid(&path, format!("index out of range (volume {volume})")));
            }
            if let Some(data) = v.as_compound().and_then(|c| c.get("block_entity_data")).and_then(Tag::as_compound) {
                block_entities.push(BlockEntity { pos: pos_of(i, plane, sz as usize), data: data.clone() });
            }
        }
        block_entities.sort_by_key(|b| b.pos);
    }

    let entities = st.get("entities").and_then(Tag::as_list).map_or_else(Vec::new, |l| l.iter().filter_map(Tag::as_compound).cloned().collect());

    Ok(Structure { edition: Edition::Bedrock, format: Format::McStructure, format_version, size, origin, palette, blocks, block_entities, entities })
}

/// Linear index → position (z varies fastest, then y, then x).
fn pos_of(i: usize, plane: usize, sz: usize) -> [u32; 3] {
    let x = i / plane;
    let y = (i % plane) / sz;
    let z = i % sz;
    [x, y, z].map(|v| u32::try_from(v).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_index_order_is_z_fastest() {
        // 4×17×14: 167 → (0, 11, 13)
        assert_eq!(pos_of(167, 17 * 14, 14), [0, 11, 13]);
        assert_eq!(pos_of(0, 17 * 14, 14), [0, 0, 0]);
        assert_eq!(pos_of(17 * 14, 17 * 14, 14), [1, 0, 0]);
        assert_eq!(pos_of(14, 17 * 14, 14), [0, 1, 0]);
    }
}
