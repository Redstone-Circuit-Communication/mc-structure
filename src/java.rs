//! Java vanilla structure (the `.nbt` written by a structure block's "Save"): gzip-compressed big-endian NBT.
//!
//! ```text
//! DataVersion: Int
//! size: List<Int>[3]
//! palette: List<{Name, Properties?: {k: String}}>      or palettes: List<List<…>> (random variants; the first is used)
//! blocks: List<{pos: List<Int>[3], state: Int, nbt?: Compound}>
//! entities: List<{pos: List<Double>[3], blockPos: List<Int>[3], nbt: Compound}>
//! ```
//!
//! `.litematic` (Litematica schematic) is gzip Java NBT as well, laid out differently (regions, bit-packed BlockStates); see the
//! `litematic` module.
//! Once the root tag is read, the format is decided **by content**, not by extension.

use std::io::Read;

use crate::bedrock::size_of;
use crate::litematic;
use crate::nbt::{self, Compound, Endian, Tag};
use crate::{Block, BlockEntity, BlockState, Edition, Error, Format, Limits, StateValue, Structure, is_compressed};

/// gzip, zlib or uncompressed. Decompression is capped: anything over the limit is rejected rather than exhausting memory.
fn inflate(bytes: &[u8], limit: u64) -> Result<Vec<u8>, Error> {
    if !is_compressed(bytes) {
        return Ok(bytes.to_vec());
    }
    let mut out = Vec::new();
    let r = if bytes.starts_with(&[0x1f, 0x8b]) {
        flate2::read::GzDecoder::new(bytes).take(limit.saturating_add(1)).read_to_end(&mut out)
    } else {
        flate2::read::ZlibDecoder::new(bytes).take(limit.saturating_add(1)).read_to_end(&mut out)
    };
    r.map_err(|e| Error::Inflate(e.to_string()))?;
    if out.len() as u64 > limit {
        return Err(Error::TooLarge(format!("decompressed size exceeds {limit} bytes")));
    }
    Ok(out)
}

/// Palette (a vanilla structure's `palette` and a schematic region's `BlockStatePalette` share the same layout).
pub(crate) fn palette_from(list: &[Tag], path: &str) -> Result<Vec<BlockState>, Error> {
    let mut out = Vec::with_capacity(list.len());
    for (i, e) in list.iter().enumerate() {
        let p = format!("{path}[{i}]");
        let e = e.as_compound().ok_or_else(|| Error::invalid(&p, "not a compound tag"))?;
        let name = e.get("Name").and_then(Tag::as_str).ok_or_else(|| Error::invalid(&p, "missing Name"))?;
        let mut states = Vec::new();
        if let Some(props) = e.get("Properties").and_then(Tag::as_compound) {
            for (k, v) in props.iter() {
                let v = v.as_str().ok_or_else(|| Error::invalid(&format!("{p}.Properties.{k}"), "property value should be a string"))?;
                states.push((k.to_owned(), StateValue::String(v.to_owned())));
            }
        }
        out.push(BlockState::new(name, states));
    }
    Ok(out)
}

pub(crate) fn parse(bytes: &[u8], limits: &Limits) -> Result<Structure, Error> {
    let raw = inflate(bytes, limits.max_inflated)?;
    let (_, root) = nbt::read(&raw, Endian::Big, limits)?;
    // Decide by content: a schematic has Regions; a vanilla structure has at least one of size / blocks / palette(s), and a
    // missing part is reported by name rather than as an unrecognized file; with none of them it is some other format.
    if litematic::is_litematic(&root) {
        return litematic::from_root(&root, limits);
    }
    if !["size", "blocks", "palette", "palettes"].iter().any(|k| root.get(k).is_some()) {
        return Err(Error::UnknownFormat);
    }
    let format_version = root.get("DataVersion").and_then(Tag::as_i32).unwrap_or(0);
    let size = size_of(&root, "size", limits)?;

    let palette = match (root.get("palette").and_then(Tag::as_list), root.get("palettes").and_then(Tag::as_list)) {
        (Some(p), _) => palette_from(p, "palette")?,
        (None, Some(ps)) => {
            let first = ps.first().and_then(Tag::as_list).ok_or_else(|| Error::invalid("palettes", "empty"))?;
            palette_from(first, "palettes[0]")?
        }
        (None, None) => return Err(Error::invalid("palette", "missing palette")),
    };

    let list = root.get("blocks").and_then(Tag::as_list).ok_or_else(|| Error::invalid("blocks", "missing"))?;
    let mut blocks = Vec::with_capacity(list.len());
    let mut block_entities = Vec::new();
    for (i, b) in list.iter().enumerate() {
        let path = format!("blocks[{i}]");
        let b: &Compound = b.as_compound().ok_or_else(|| Error::invalid(&path, "not a compound tag"))?;
        let pos = b.get("pos").and_then(Tag::as_ints).ok_or_else(|| Error::invalid(&format!("{path}.pos"), "missing"))?;
        let pos = <[i32; 3]>::try_from(pos.as_slice()).map_err(|_| Error::invalid(&format!("{path}.pos"), "expected three integers"))?;
        let mut p = [0u32; 3];
        for ((o, v), max) in p.iter_mut().zip(pos).zip(size) {
            *o = u32::try_from(v).ok().filter(|v| *v < max).ok_or_else(|| Error::invalid(&format!("{path}.pos"), format!("position {pos:?} out of bounds")))?;
        }
        let state = b.get("state").and_then(Tag::as_i32).ok_or_else(|| Error::invalid(&format!("{path}.state"), "missing"))?;
        let st = usize::try_from(state).ok().and_then(|s| palette.get(s)).ok_or_else(|| {
            Error::invalid(&format!("{path}.state"), format!("palette index {state} out of range (the palette has {} entries)", palette.len()))
        })?;
        if let Some(data) = b.get("nbt").and_then(Tag::as_compound) {
            block_entities.push(BlockEntity { pos: p, data: data.clone() });
        }
        if !st.is_empty() {
            blocks.push(Block { pos: p, state: state.unsigned_abs(), extra: None });
        }
    }
    blocks.sort_by_key(|b| b.pos);
    // A cell listed twice (a file broken by some tool): keep the first one
    blocks.dedup_by_key(|b| b.pos);
    block_entities.sort_by_key(|b| b.pos);

    let entities = root.get("entities").and_then(Tag::as_list).map_or_else(Vec::new, |l| l.iter().filter_map(Tag::as_compound).cloned().collect());

    Ok(Structure { edition: Edition::Java, format: Format::JavaStructure, format_version, size, origin: None, palette, blocks, block_entities, entities })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn s(v: &str) -> Tag {
        Tag::String(v.into())
    }

    fn ints(v: &[i32]) -> Tag {
        Tag::List(v.iter().map(|x| Tag::Int(*x)).collect())
    }

    /// Builds a minimal Java structure: 2×1×1, one cell of air and one sticky piston (with a block entity).
    pub(crate) fn sample() -> Vec<u8> {
        let piston = Compound(vec![("facing".into(), s("up")), ("extended".into(), s("false"))]);
        let root = Compound(vec![
            ("DataVersion".into(), Tag::Int(3953)),
            ("size".into(), ints(&[2, 1, 1])),
            (
                "palette".into(),
                Tag::List(vec![
                    Tag::Compound(Compound(vec![("Name".into(), s("minecraft:air"))])),
                    Tag::Compound(Compound(vec![("Name".into(), s("minecraft:sticky_piston")), ("Properties".into(), Tag::Compound(piston))])),
                ]),
            ),
            (
                "blocks".into(),
                Tag::List(vec![
                    Tag::Compound(Compound(vec![("pos".into(), ints(&[0, 0, 0])), ("state".into(), Tag::Int(0))])),
                    Tag::Compound(Compound(vec![
                        ("pos".into(), ints(&[1, 0, 0])),
                        ("state".into(), Tag::Int(1)),
                        ("nbt".into(), Tag::Compound(Compound(vec![("id".into(), s("minecraft:piston"))]))),
                    ])),
                ]),
            ),
            ("entities".into(), Tag::List(vec![])),
        ]);
        let nbt = nbt::write("", &root, Endian::Big);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&nbt).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn parses_gzipped_structure() {
        let st = parse(&sample(), &Limits::default()).unwrap();
        assert_eq!(st.edition, Edition::Java);
        assert_eq!(st.format_version, 3953);
        assert_eq!(st.size, [2, 1, 1]);
        assert_eq!(st.block_count(), 1, "air is not counted");
        assert_eq!(st.cell_key(&st.blocks[0]), "minecraft:sticky_piston[extended=false,facing=up]");
        assert_eq!(st.block_entities.len(), 1);
        assert_eq!(st.block_entities[0].pos, [1, 0, 0]);
    }

    #[test]
    fn same_structure_same_digest_regardless_of_compression() {
        let gz = sample();
        let mut raw = Vec::new();
        flate2::read::GzDecoder::new(gz.as_slice()).read_to_end(&mut raw).unwrap();
        let a = parse(&gz, &Limits::default()).unwrap();
        let b = parse(&raw, &Limits::default()).unwrap();
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn inflate_bomb_is_capped() {
        let zeros = vec![0u8; 1024 * 1024];
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
        gz.write_all(&zeros).unwrap();
        let bomb = gz.finish().unwrap();
        let limits = Limits { max_inflated: 64 * 1024, ..Limits::default() };
        assert!(matches!(parse(&bomb, &limits), Err(Error::TooLarge(_))));
    }

    #[test]
    fn other_java_nbt_is_unknown_but_a_partial_structure_says_what_is_missing() {
        // .schem and the like: neither a vanilla structure nor a schematic
        let schem = Compound(vec![("Version".into(), Tag::Int(2)), ("Width".into(), Tag::Short(1)), ("Palette".into(), Tag::Compound(Compound(vec![])))]);
        let e = parse(&nbt::write("", &schem, Endian::Big), &Limits::default()).unwrap_err();
        assert!(matches!(e, Error::UnknownFormat), "{e}");
        // A vanilla structure with a missing part: the error still names that part
        let partial = Compound(vec![("size".into(), ints(&[1, 1, 1]))]);
        let e = parse(&nbt::write("", &partial, Endian::Big), &Limits::default()).unwrap_err();
        assert!(e.to_string().contains("palette"), "{e}");
    }

    #[test]
    fn out_of_range_palette_index_is_an_error() {
        let root = Compound(vec![
            ("size".into(), ints(&[1, 1, 1])),
            ("palette".into(), Tag::List(vec![Tag::Compound(Compound(vec![("Name".into(), s("minecraft:stone"))]))])),
            ("blocks".into(), Tag::List(vec![Tag::Compound(Compound(vec![("pos".into(), ints(&[0, 0, 0])), ("state".into(), Tag::Int(5))]))])),
        ]);
        let bytes = nbt::write("", &root, Endian::Big);
        let e = parse(&bytes, &Limits::default()).unwrap_err();
        assert!(e.to_string().contains("blocks[0].state"), "{e}");
    }
}
