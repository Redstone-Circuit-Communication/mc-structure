//! Tests for Litematica schematics (`.litematic`). Fixtures and expected values come from an independent Python reference
//! implementation (`tools/litematic.py`, which shares no code with this crate: it treats the whole long array as one big integer
//! and slices bits from it, while this crate shifts and joins neighbouring longs).
//! Schematics are also built on the fly, with a packer that places one bit at a time (nothing like the crate's own code), to
//! cover many palette sizes and malformed files.

// clippy.toml's allow-*-in-tests covers `#[test]` functions and `#[cfg(test)]` modules only; helper functions in integration
// test files (load, bytes_of, gzip, …) are not "in tests" for clippy 1.94. Test code should panic on failure, so the whole file opts out.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::io::{Read, Write};

use mc_structure::nbt::{self, Compound, Endian, Tag};
use mc_structure::{Edition, Error, Format, Limits, Structure, parse};

fn bytes_of(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn load(name: &str) -> Structure {
    parse(&bytes_of(name), Some(name), &Limits::default()).unwrap()
}

fn top(s: &Structure, n: usize) -> Vec<(String, u32)> {
    s.materials().into_iter().take(n).collect()
}

fn pairs(v: &[(&str, u32)]) -> Vec<(String, u32)> {
    v.iter().map(|(k, n)| ((*k).to_owned(), *n)).collect()
}

// ───────────────────────── Fixtures: every expected value comes from tools/litematic.py

#[test]
fn door_is_the_same_structure_in_both_formats() {
    for state in ["close", "open"] {
        let nbt = load(&format!("door_{state}.nbt"));
        let lit = load(&format!("door_{state}.litematic"));
        assert_eq!(nbt.format, Format::JavaStructure);
        assert_eq!(lit.format, Format::Litematic);
        assert_eq!(lit.edition, Edition::Java);
        assert_eq!(lit.size, nbt.size);
        assert_eq!(lit.block_count(), nbt.block_count());
        assert_eq!(lit.digest(), nbt.digest(), "{state}: palette order, version and negative sizes must not change the digest");
        assert_eq!(lit.materials(), nbt.materials());
    }
}

#[test]
fn door_golden_values() {
    // Closed state: version 6, positive size, Position away from the origin
    let s = load("door_close.litematic");
    assert_eq!(s.format_version, 3953);
    assert_eq!(s.size, [5, 4, 3]);
    assert_eq!(s.block_count(), 24);
    assert_eq!(hex::encode(s.digest()), "fef504d3bec3fd5d6db0bc69bc4318b52243ecf41a8d139d7f0b5076fdfbad33");
    let want = pairs(&[
        ("minecraft:stone", 15),
        ("minecraft:quartz_block", 4),
        ("minecraft:sticky_piston", 3),
        ("minecraft:observer", 1),
        ("minecraft:redstone_wire", 1),
    ]);
    assert_eq!(top(&s, 5), want);
    // Open state: version 7 (with SubVersion), negative Size on all three axes
    let s = load("door_open.litematic");
    assert_eq!(s.size, [5, 4, 3]);
    assert_eq!(s.block_count(), 27);
    assert_eq!(hex::encode(s.digest()), "abfca88e375a430515f38f9162cf8c73cd6ef98cb0e799f770cee32370ad2aa7");
    let want = pairs(&[
        ("minecraft:stone", 15),
        ("minecraft:quartz_block", 4),
        ("minecraft:piston_head", 3),
        ("minecraft:sticky_piston", 3),
        ("minecraft:observer", 1),
    ]);
    assert_eq!(top(&s, 5), want);
    // Difference between the states: three pistons change state, three piston heads, redstone wire, an observer
    let d = load("door_close.litematic").diff(&s);
    assert_eq!((d.moving, d.total), (8, 27));
}

#[test]
fn door_blocks_are_sorted_and_use_the_unified_palette() {
    let s = load("door_close.litematic");
    assert!(s.blocks.windows(2).all(|w| w[0].pos < w[1].pos), "strictly ascending by position");
    assert!(s.blocks.iter().all(|b| b.extra.is_none()));
    assert!(s.blocks.iter().all(|b| (b.state as usize) < s.palette.len()));
    // The bottom corner (0,0,0) is the stone floor
    assert_eq!(s.blocks[0].pos, [0, 0, 0]);
    assert_eq!(s.cell_key(&s.blocks[0]), "minecraft:stone");
    assert_eq!(s.origin, None);
}

#[test]
fn multi_region_with_overlap_negative_size_and_lying_metadata() {
    let s = load("multi_region.litematic");
    // Metadata claims TotalBlocks = 1 and EnclosingSize = 1×1×1; it is ignored in favour of the decoded data
    assert_eq!(s.size, [8, 6, 4]);
    assert_eq!(s.block_count(), 29);
    assert_eq!(hex::encode(s.digest()), "719c8d597fb4ab1fed0a1cebf1a945ff435b321dc9ceb6a4fd886dcae085b2ab");
    assert_eq!(
        top(&s, 5),
        pairs(&[("minecraft:obsidian", 10), ("minecraft:stone", 9), ("minecraft:quartz_block", 6), ("minecraft:dirt", 2), ("minecraft:chest", 1)])
    );
    // wall and base overlap at x = 3: the later region wins, so three stone blocks of base are replaced by quartz (9 stone left)
    let at = |p: [u32; 3]| s.blocks.iter().find(|b| b.pos == p).map(|b| s.cell_key(b));
    assert_eq!(at([3, 0, 1]).as_deref(), Some("minecraft:quartz_block"));
    assert_eq!(at([2, 0, 1]).as_deref(), Some("minecraft:stone"));
    // The neg region (negative Size) has its minimum corner at (6, 3, 2); its cell (0,0,0) is a chest
    assert_eq!(at([6, 3, 2]).as_deref(), Some("minecraft:chest[facing=north,type=single,waterlogged=false]"));
    // The same structure flattened into a vanilla structure has the same digest
    let flat = load("multi_region_flat.nbt");
    assert_eq!(flat.digest(), s.digest());
    assert_eq!(flat.block_entities.len(), s.block_entities.len());
}

#[test]
fn multi_region_block_entities_and_entities_get_the_region_offset() {
    let s = load("multi_region.litematic");
    // Block entities: positions get the region offset (wall (1,1,1) → (4,1,1), neg (0,0,0) → (6,3,2)) and the region-relative
    // x, y, z are dropped from the data
    let positions: Vec<[u32; 3]> = s.block_entities.iter().map(|b| b.pos).collect();
    assert_eq!(positions, [[4, 1, 1], [6, 3, 2]]);
    let chest = s.block_entities.iter().find(|b| b.pos == [6, 3, 2]).unwrap();
    assert_eq!(chest.data.get("id").and_then(Tag::as_str), Some("minecraft:chest"));
    assert!(chest.data.get("x").is_none() && chest.data.get("y").is_none() && chest.data.get("z").is_none());
    // Entities: Pos is relative to the region and gets the region offset (neg starts at (6, 3, 2)); other keys stay as they are
    assert_eq!(s.entities.len(), 1);
    let pos: Vec<f64> = s.entities[0].get("Pos").and_then(Tag::as_list).unwrap().iter().map(|t| if let Tag::Double(d) = t { *d } else { f64::NAN }).collect();
    assert_eq!(pos, [6.5, 3.0, 2.5]);
    assert_eq!(s.entities[0].get("id").and_then(Tag::as_str), Some("minecraft:armor_stand"));
    assert!(s.entities[0].get("Motion").and_then(Tag::as_list).is_some());
}

#[test]
fn large_region_with_block_entities() {
    // Generated by tools/litematic.py: one region of 40×20×36, a palette of 700 entries (10 bits per index, spanning long
    // boundaries), 390 block entities on chests, barrels and signs, and truthful metadata
    let s = load("large_region.litematic");
    assert_eq!((s.format, s.format_version, s.size), (Format::Litematic, 3953, [40, 20, 36]));
    // Matches TotalBlocks (15872) and EnclosingSize (40×20×36) in the metadata
    assert_eq!(s.block_count(), 15872);
    assert_eq!(hex::encode(s.digest()), "9f7a8a23b8db0ca7e9758048e7cfc277068b210616c46220a1eb615d931d51b6");
    let want = pairs(&[("minecraft:note_block", 15482), ("minecraft:oak_sign", 191), ("minecraft:barrel", 144), ("minecraft:chest", 55)]);
    assert_eq!(top(&s, 5), want);
    assert_eq!((s.block_entities.len(), s.entities.len()), (390, 0));
    // Checks the cell order against the block entity positions, which the file records separately: with the right order every
    // one of them lands on a chest, barrel or sign; with a wrong order (x and y swapped, say) most land on air or note blocks
    let at: std::collections::HashMap<[u32; 3], &str> = s.blocks.iter().map(|b| (b.pos, s.palette[b.state as usize].name.as_str())).collect();
    let misplaced =
        s.block_entities.iter().filter(|be| !matches!(at.get(&be.pos).copied(), Some("minecraft:chest" | "minecraft:barrel" | "minecraft:oak_sign"))).count();
    assert_eq!(misplaced, 0, "block entities not on their blocks: wrong cell order");
}

#[test]
fn palettes_that_cross_long_boundaries() {
    // 17 entries → 5 bits per index (105 indices = 525 bits, crossing long boundaries many times)
    let s = load("palette17.litematic");
    assert_eq!((s.size, s.block_count()), ([7, 5, 3], 98));
    assert_eq!(hex::encode(s.digest()), "ddea78ff5bd9cb157fbe743d15836941045fd5434505b55067d5d28a043bcb8c");
    // Exactly 64 entries (2^6) → 6 bits; 64 is not a multiple of 6, so indices cross boundaries too
    let s = load("palette64.litematic");
    assert_eq!((s.size, s.block_count()), ([5, 4, 4], 79));
    assert_eq!(hex::encode(s.digest()), "be74d993a9eb2d66a3a8f6928ffbb1fcd7bd452be5ec1d0206bc30aa3304afc4");
}

#[test]
fn parsed_by_content_not_by_extension() {
    // A schematic renamed to .nbt: parsed by content, and the format reports the content
    let lit = bytes_of("door_close.litematic");
    let s = parse(&lit, Some("door.nbt"), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::Litematic);
    // The other way round: a vanilla structure renamed to .litematic
    let nbt = bytes_of("door_close.nbt");
    let s = parse(&nbt, Some("door.litematic"), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::JavaStructure);
    // Works without a file name too (gzip always means Java)
    assert_eq!(parse(&lit, None, &Limits::default()).unwrap().format, Format::Litematic);
}

#[test]
fn uncompressed_litematic_needs_the_extension_to_be_recognised() {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(bytes_of("door_close.litematic").as_slice()).read_to_end(&mut raw).unwrap();
    let s = parse(&raw, Some("door.litematic"), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::Litematic);
    assert_eq!(hex::encode(s.digest()), "fef504d3bec3fd5d6db0bc69bc4318b52243ecf41a8d139d7f0b5076fdfbad33");
    // Uncompressed and without an extension: the edition cannot be told, so no guess is made
    assert!(matches!(parse(&raw, None, &Limits::default()), Err(Error::UnknownFormat)));
}

#[test]
fn format_names_are_the_file_extensions() {
    assert_eq!(Format::JavaStructure.as_str(), "nbt");
    assert_eq!(Format::Litematic.as_str(), "litematic");
    assert_eq!(Format::McStructure.as_str(), "mcstructure");
}

#[test]
fn inflate_limit_applies_to_litematic_too() {
    let limits = Limits { max_inflated: 100, ..Limits::default() };
    let e = parse(&bytes_of("door_close.litematic"), Some("x.litematic"), &limits).unwrap_err();
    assert!(matches!(e, Error::TooLarge(_)), "{e}");
}

// ───────────────────────── Schematics built on the fly

fn s(v: &str) -> Tag {
    Tag::String(v.into())
}

fn compound(parts: Vec<(&str, Tag)>) -> Tag {
    Tag::Compound(Compound(parts.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()))
}

fn xyz(x: i32, y: i32, z: i32) -> Tag {
    compound(vec![("x", Tag::Int(x)), ("y", Tag::Int(y)), ("z", Tag::Int(z))])
}

fn palette_tag(names: &[&str]) -> Tag {
    Tag::List(names.iter().map(|n| compound(vec![("Name", s(n))])).collect())
}

/// Bits per index, `max(2, ceil(log2(n)))`, computed with a loop (not the leading-zeros approach of the crate).
fn bits_of(palette_len: usize) -> u32 {
    let mut bits = 2;
    while (1usize << bits) < palette_len {
        bits += 1;
    }
    bits
}

/// Tight packing: index i starts at bit i·bits and continues across long boundaries; bit 0 is the lowest bit of the first long.
/// Places one bit at a time, without any shift-and-join, so it shares nothing with the crate's implementation.
fn pack(indices: &[u64], bits: u32) -> Vec<i64> {
    let mut stream: Vec<bool> = Vec::new();
    for v in indices {
        for b in 0..bits {
            stream.push((v >> b) & 1 == 1);
        }
    }
    stream
        .chunks(64)
        .map(|chunk| {
            let mut word = 0u64;
            for (k, bit) in chunk.iter().enumerate() {
                if *bit {
                    word |= 1 << k;
                }
            }
            i64::from_ne_bytes(word.to_ne_bytes())
        })
        .collect()
}

fn region_with(position: [i32; 3], size: [i32; 3], palette: Tag, states: Tag) -> Tag {
    compound(vec![
        ("Position", xyz(position[0], position[1], position[2])),
        ("Size", xyz(size[0], size[1], size[2])),
        ("BlockStatePalette", palette),
        ("BlockStates", states),
    ])
}

fn region(position: [i32; 3], size: [i32; 3], palette: &[&str], indices: &[u64]) -> Tag {
    region_with(position, size, palette_tag(palette), Tag::LongArray(pack(indices, bits_of(palette.len()))))
}

fn gzip(raw: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(raw).unwrap();
    gz.finish().unwrap()
}

fn file_from(root: Compound) -> Vec<u8> {
    gzip(&nbt::write("", &root, Endian::Big))
}

fn file(regions: Vec<(String, Tag)>) -> Vec<u8> {
    file_from(Compound(vec![
        ("MinecraftDataVersion".into(), Tag::Int(3953)),
        ("Version".into(), Tag::Int(6)),
        ("Metadata".into(), Tag::Compound(Compound(vec![]))),
        ("Regions".into(), Tag::Compound(Compound(regions))),
    ]))
}

fn one(r: Tag) -> Vec<u8> {
    file(vec![("r".to_owned(), r)])
}

fn parse_default(bytes: &[u8]) -> Result<Structure, Error> {
    parse(bytes, Some("t.litematic"), &Limits::default())
}

#[test]
fn every_palette_size_round_trips_through_an_independent_packer() {
    let (nx, ny, nz) = (6usize, 5usize, 4usize);
    let volume = nx * ny * nz;
    for n in [1usize, 2, 3, 4, 5, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 127, 128, 129, 255, 256, 257, 300, 1000] {
        let names: Vec<String> = (0..n).map(|k| if k == 0 { "minecraft:air".to_owned() } else { format!("minecraft:b{k}") }).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let indices: Vec<u64> = (0..volume).map(|i| ((i * 7 + 3) % n) as u64).collect();
        // Expected: every cell whose index is not 0 (air); x varies fastest, then z, then y
        let mut want: Vec<([u32; 3], String)> = Vec::new();
        for (i, v) in indices.iter().enumerate() {
            if *v != 0 {
                let (x, z, y) = (i % nx, (i / nx) % nz, i / (nx * nz));
                want.push(([x as u32, y as u32, z as u32], format!("minecraft:b{v}")));
            }
        }
        want.sort();
        // A positive size and the same region described with negative Size and Position at the maximum corner must read the same
        for (position, size) in [([0, 0, 0], [6, 5, 4]), ([9, 8, 7], [-6, -5, -4])] {
            let st = parse_default(&one(region(position, size, &refs, &indices))).unwrap();
            assert_eq!(st.size, [6, 5, 4], "palette of {n} entries");
            let got: Vec<([u32; 3], String)> = st.cells().collect();
            assert_eq!(got, want, "palette of {n} entries, Size {size:?}");
        }
    }
}

#[test]
fn regions_merge_and_air_never_overrides() {
    // a: 1×1×1 of stone; b: same cell, air → the stone stays. c: same cell, dirt → replaces the stone
    let air_stone = ["minecraft:air", "minecraft:stone"];
    let air_dirt = ["minecraft:air", "minecraft:dirt"];
    let a = region([0, 0, 0], [1, 1, 1], &air_stone, &[1]);
    let b = region([0, 0, 0], [1, 1, 1], &air_dirt, &[0]);
    let c = region([0, 0, 0], [1, 1, 1], &air_dirt, &[1]);
    let st = parse_default(&file(vec![("a".into(), a.clone()), ("b".into(), b.clone())])).unwrap();
    assert_eq!(st.cells().collect::<Vec<_>>(), [([0, 0, 0], "minecraft:stone".to_owned())]);
    let st = parse_default(&file(vec![("a".into(), a), ("b".into(), b), ("c".into(), c.clone())])).unwrap();
    assert_eq!(st.cells().collect::<Vec<_>>(), [([0, 0, 0], "minecraft:dirt".to_owned())]);
    // Merging the three palettes: air is in all three, dirt in b and c; the merged palette has each once
    assert_eq!(st.palette.len(), 3, "air, stone and dirt once each");
    assert_eq!(st.palette.iter().filter(|p| p.name == "minecraft:dirt").count(), 1);
    // Reversed order: dirt first, stone last, so the stone replaces the dirt
    let a2 = region([0, 0, 0], [1, 1, 1], &air_stone, &[1]);
    let st = parse_default(&file(vec![("c".into(), c), ("a".into(), a2)])).unwrap();
    assert_eq!(st.cells().collect::<Vec<_>>(), [([0, 0, 0], "minecraft:stone".to_owned())]);
}

#[test]
fn regions_far_apart_take_the_bounding_box() {
    // Two 1×1×1 regions, one at the origin and one at (5, -2, 3): the bounds are 6×3×4 with the minimum corner moved to 0
    let pal = ["minecraft:air", "minecraft:stone"];
    let a = region([0, 0, 0], [1, 1, 1], &pal, &[1]);
    let b = region([5, -2, 3], [1, 1, 1], &pal, &[1]);
    let st = parse_default(&file(vec![("a".into(), a), ("b".into(), b)])).unwrap();
    assert_eq!(st.size, [6, 3, 4]);
    let cells: Vec<[u32; 3]> = st.cells().map(|(p, _)| p).collect();
    assert_eq!(cells, [[0, 2, 0], [5, 0, 3]]);
}

// ───────────────────────── Malformed files

fn expect_invalid(bytes: &[u8], needle: &str) {
    let e = parse_default(bytes).unwrap_err();
    assert!(matches!(e, Error::Invalid { .. }), "{e}");
    assert!(e.to_string().contains(needle), "{e}");
}

#[test]
fn short_block_states_is_an_error_but_extra_longs_are_ignored() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let good = pack(&[1u64; 24], 2);
    // Too few longs means missing data
    let mut short = good.clone();
    short.pop();
    for longs in [short, vec![]] {
        let bytes = one(region_with([0, 0, 0], [4, 3, 2], palette_tag(&pal), Tag::LongArray(longs)));
        expect_invalid(&bytes, "Regions.r.BlockStates");
    }
    // Extra longs are ignored: Litematica writes exactly as many as needed, other tools may pad with zeros
    let mut extra = good;
    extra.push(0);
    let st = parse_default(&one(region_with([0, 0, 0], [4, 3, 2], palette_tag(&pal), Tag::LongArray(extra)))).unwrap();
    assert_eq!(st.block_count(), 24);
}

#[test]
fn block_entities_key_is_accepted_too() {
    // The block entity key is usually TileEntities, sometimes BlockEntities
    let pal = ["minecraft:air", "minecraft:chest"];
    let Tag::Compound(Compound(mut parts)) = region([5, 0, 0], [2, 1, 1], &pal, &[0, 1]) else { unreachable!() };
    let chest = compound(vec![("x", Tag::Int(1)), ("y", Tag::Int(0)), ("z", Tag::Int(0)), ("id", s("minecraft:chest"))]);
    parts.push(("BlockEntities".into(), Tag::List(vec![chest])));
    let st = parse_default(&one(Tag::Compound(Compound(parts)))).unwrap();
    assert_eq!(st.block_entities.len(), 1);
    assert_eq!(st.block_entities[0].pos, [1, 0, 0]);
    assert_eq!(st.block_entities[0].data.get("id").and_then(Tag::as_str), Some("minecraft:chest"));
}

#[test]
fn palette_index_out_of_range_is_an_error() {
    // A palette of 3 entries (2 bits, so index 3 can be written), with one cell set to 3
    let pal = ["minecraft:air", "minecraft:stone", "minecraft:dirt"];
    let bytes = one(region_with([0, 0, 0], [2, 1, 1], palette_tag(&pal), Tag::LongArray(pack(&[1, 3], 2))));
    expect_invalid(&bytes, "out of range");
}

#[test]
fn missing_or_wrong_typed_parts_are_errors() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let long = || Tag::LongArray(pack(&[1], 2));
    // No palette, an empty palette, BlockStates not a LongArray, Position missing, Size missing
    let no_palette = compound(vec![("Position", xyz(0, 0, 0)), ("Size", xyz(1, 1, 1)), ("BlockStates", long())]);
    expect_invalid(&one(no_palette), "Regions.r.BlockStatePalette");
    expect_invalid(&one(region_with([0, 0, 0], [1, 1, 1], Tag::List(vec![]), long())), "palette is empty");
    expect_invalid(&one(region_with([0, 0, 0], [1, 1, 1], palette_tag(&pal), Tag::IntArray(vec![1]))), "LongArray");
    let no_pos = compound(vec![("Size", xyz(1, 1, 1)), ("BlockStatePalette", palette_tag(&pal)), ("BlockStates", long())]);
    expect_invalid(&one(no_pos), "Regions.r.Position");
    let no_size = compound(vec![("Position", xyz(0, 0, 0)), ("BlockStatePalette", palette_tag(&pal)), ("BlockStates", long())]);
    expect_invalid(&one(no_size), "Regions.r.Size");
    // A region that is not a compound; no regions at all
    expect_invalid(&file(vec![("r".into(), Tag::Int(1))]), "Regions.r");
    expect_invalid(&file(vec![]), "no regions");
}

#[test]
fn tile_entity_outside_its_region_is_an_error() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let Tag::Compound(Compound(mut parts)) = region([0, 0, 0], [1, 1, 1], &pal, &[1]) else { unreachable!() };
    let outside = compound(vec![("x", Tag::Int(5)), ("y", Tag::Int(0)), ("z", Tag::Int(0)), ("id", s("minecraft:chest"))]);
    parts.push(("TileEntities".into(), Tag::List(vec![outside])));
    expect_invalid(&one(Tag::Compound(Compound(parts))), "TileEntities[0].x");
}

#[test]
fn oversized_regions_are_rejected_before_decoding() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let too_large = |bytes: Vec<u8>, limits: &Limits| {
        let e = parse(&bytes, Some("t.litematic"), limits).unwrap_err();
        assert!(matches!(e, Error::TooLarge(_)), "{e}");
    };
    let limits = Limits::default();
    // An axis over the limit, a size of 0 (an empty BlockStates is neither allocated nor decoded)
    too_large(one(region_with([0, 0, 0], [20_000, 1, 1], palette_tag(&pal), Tag::LongArray(vec![]))), &limits);
    too_large(one(region_with([0, 0, 0], [0, 1, 1], palette_tag(&pal), Tag::LongArray(vec![]))), &limits);
    too_large(one(region_with([0, 0, 0], [i32::MIN, 1, 1], palette_tag(&pal), Tag::LongArray(vec![]))), &limits);
    // Volume over the element limit
    let small = Limits { max_elements: 100, ..Limits::default() };
    too_large(one(region_with([0, 0, 0], [10, 10, 2], palette_tag(&pal), Tag::LongArray(vec![]))), &small);
    // Each region is within the limit but together they exceed it (6×6×2 = 72, two of them 144 > 100)
    let r = || region_with([0, 0, 0], [6, 6, 2], palette_tag(&pal), Tag::LongArray(pack(&[1u64; 72], 2)));
    too_large(file(vec![("a".into(), r()), ("b".into(), r())]), &small);
    // Two distant regions: each small, but their combined bounds exceed the axis limit
    let far = file(vec![("a".into(), region([0, 0, 0], [1, 1, 1], &pal, &[1])), ("b".into(), region([20_000, 0, 0], [1, 1, 1], &pal, &[1]))]);
    too_large(far, &limits);
}

#[test]
fn too_many_regions_are_rejected() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let many = |n: usize| file((0..n).map(|i| (format!("r{i}"), region([i as i32, 0, 0], [1, 1, 1], &pal, &[1]))).collect());
    assert_eq!(parse_default(&many(64)).unwrap().block_count(), 64);
    assert!(matches!(parse_default(&many(65)), Err(Error::TooLarge(_))));
}

#[test]
fn other_java_nbt_is_not_a_litematic() {
    // Regions that is not a compound and none of the vanilla structure keys: unrecognized
    let root = Compound(vec![("Regions".into(), Tag::List(vec![])), ("MinecraftDataVersion".into(), Tag::Int(1))]);
    assert!(matches!(parse_default(&file_from(root)), Err(Error::UnknownFormat)));
    // The root of a Sponge .schem
    let schem = Compound(vec![("Version".into(), Tag::Int(2)), ("Width".into(), Tag::Short(1)), ("Palette".into(), Tag::Compound(Compound(vec![])))]);
    assert!(matches!(parse_default(&file_from(schem)), Err(Error::UnknownFormat)));
}

#[test]
fn missing_data_version_is_tolerated_as_zero() {
    // Old schematics have no MinecraftDataVersion: still a schematic, with format_version 0
    let pal = ["minecraft:air", "minecraft:stone"];
    let root = Compound(vec![("Regions".into(), Tag::Compound(Compound(vec![("r".into(), region([0, 0, 0], [1, 1, 1], &pal, &[1]))])))]);
    let st = parse_default(&file_from(root)).unwrap();
    assert_eq!((st.format, st.format_version, st.block_count()), (Format::Litematic, 0, 1));
}
