//! Memory amplification. A file of a few KB after gzip can decompress to a byte list of four million elements: one byte each in
//! the file, but a 32-byte tag each in memory. Counting elements alone would let such a file take about 128 MB; counting bytes
//! rejects it before allocating.

// clippy.toml's allow-*-in-tests covers `#[test]` functions and `#[cfg(test)]` modules only; helper functions in integration
// test files (load, bytes_of, gzip, …) are not "in tests" for clippy 1.94. Test code should panic on failure, so the whole file opts out.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::io::Write;

use mc_structure::nbt::NbtErrorKind;
use mc_structure::{Edition, Error, Limits, parse_java};

#[test]
fn tiny_gzip_with_a_huge_byte_list_is_rejected_before_allocation() {
    let n: i32 = 4_000_000;
    let mut raw = vec![10, 0, 0, 9, 0, 1, b'l', 1];
    raw.extend_from_slice(&n.to_be_bytes());
    raw.resize(raw.len() + 4_000_000, 0);
    raw.push(0);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&raw).unwrap();
    let file = gz.finish().unwrap();
    assert!(file.len() < 8 * 1024, "only {} bytes compressed", file.len());
    match parse_java(&file, &Limits::default()) {
        Err(Error::Nbt(e)) => assert!(matches!(e.kind, NbtErrorKind::TooMuchMemory(_)), "{e}"),
        other => panic!("should be rejected before allocating: {other:?}"),
    }
}

fn gzip(raw: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(raw).unwrap();
    gz.finish().unwrap()
}

/// A root compound holding a byte list of n elements: one byte each in the file, one tag each in memory
fn byte_list(n: i32) -> Vec<u8> {
    let mut raw = vec![10, 0, 0, 9, 0, 1, b'l', 1];
    raw.extend_from_slice(&n.to_be_bytes());
    raw.resize(raw.len() + usize::try_from(n).unwrap(), 0);
    raw.push(0);
    raw
}

#[test]
fn java_files_get_a_smaller_memory_budget_than_legacy_bedrock() {
    assert!(Limits::java().max_memory < Limits::default().max_memory);
    assert_eq!(Limits::for_edition(Edition::Java), Limits::java());
    assert_eq!(Limits::for_edition(Edition::Bedrock), Limits::default());
    // 3.5 KB of gzip holding 3.6 million elements: about 116 MiB under the default budget, rejected before allocating under the
    // Java budget
    let file = gzip(&byte_list(3_600_000));
    assert!(file.len() < 8 * 1024, "only {} bytes compressed", file.len());
    match parse_java(&file, &Limits::java()) {
        Err(Error::Nbt(e)) => assert!(matches!(e.kind, NbtErrorKind::TooMuchMemory(m) if m == Limits::JAVA_MAX_MEMORY), "{e}"),
        other => panic!("the Java budget should reject this before allocating: {other:?}"),
    }
}

/// A vanilla .nbt structure filling 48×48×48 (the largest volume a structure block can save) with one block per cell must parse
/// within the Java budget
#[test]
fn a_full_vanilla_structure_fits_the_java_budget() {
    fn name(raw: &mut Vec<u8>, s: &str) {
        raw.extend_from_slice(&u16::try_from(s.len()).unwrap().to_be_bytes());
        raw.extend_from_slice(s.as_bytes());
    }
    let n = 48_i32;
    let mut raw = vec![10, 0, 0];
    // size: List<Int>[3]
    raw.push(9);
    name(&mut raw, "size");
    raw.push(3);
    raw.extend_from_slice(&3_i32.to_be_bytes());
    for _ in 0..3 {
        raw.extend_from_slice(&n.to_be_bytes());
    }
    // palette: two kinds of block
    raw.push(9);
    name(&mut raw, "palette");
    raw.push(10);
    raw.extend_from_slice(&2_i32.to_be_bytes());
    for b in ["minecraft:stone", "minecraft:redstone_block"] {
        raw.push(8);
        name(&mut raw, "Name");
        name(&mut raw, b);
        raw.push(0);
    }
    // blocks: one {pos: [x, y, z], state} per cell
    raw.push(9);
    name(&mut raw, "blocks");
    raw.push(10);
    raw.extend_from_slice(&(n * n * n).to_be_bytes());
    for y in 0..n {
        for z in 0..n {
            for x in 0..n {
                raw.push(9);
                name(&mut raw, "pos");
                raw.push(3);
                raw.extend_from_slice(&3_i32.to_be_bytes());
                for v in [x, y, z] {
                    raw.extend_from_slice(&v.to_be_bytes());
                }
                raw.push(3);
                name(&mut raw, "state");
                raw.extend_from_slice(&((x + y + z) % 2).to_be_bytes());
                raw.push(0);
            }
        }
    }
    raw.push(3);
    name(&mut raw, "DataVersion");
    raw.extend_from_slice(&3953_i32.to_be_bytes());
    raw.push(0);
    let s = parse_java(&gzip(&raw), &Limits::java()).unwrap_or_else(|e| panic!("a full 48³ vanilla structure should fit the Java budget: {e}"));
    assert_eq!(s.size, [48, 48, 48]);
    // With a quarter of the budget it no longer fits: the budget is sized to real files, not an arbitrary large number
    let half = Limits { max_memory: Limits::JAVA_MAX_MEMORY / 4, ..Limits::java() };
    assert!(parse_java(&gzip(&raw), &half).is_err(), "the budget should be sized to real files");
}
