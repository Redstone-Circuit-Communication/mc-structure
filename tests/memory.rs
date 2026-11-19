//! 评审 M1：内存放大。一个 gzip 后只有几 KB 的文件，解压出来是一个 400 万元素的字节列表——线上每个元素 1 字节，
//! 在内存里是一个 32 字节的标签。以前按元素个数记账，这样的文件能占到约 128 MB；现在按字节记，分配之前就拒绝。

// clippy.toml 的 allow-*-in-tests 只管 `#[test]` 函数和 `#[cfg(test)]` 模块；集成测试文件里的辅助函数（load、bytes_of、gzip……）
// 在 clippy 1.94 里不算「测试里」，会被 unwrap_used 等拦下。测试代码本来就该在出错时直接 panic，这里整个文件放行。
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
    assert!(file.len() < 8 * 1024, "压缩后只有 {} 字节", file.len());
    match parse_java(&file, &Limits::default()) {
        Err(Error::Nbt(e)) => assert!(matches!(e.kind, NbtErrorKind::TooMuchMemory(_)), "{e}"),
        other => panic!("应该在分配之前就拒绝：{other:?}"),
    }
}

fn gzip(raw: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(raw).unwrap();
    gz.finish().unwrap()
}

/// 根复合标签里放一个 n 个元素的字节列表：线上每个元素 1 字节，内存里是一个标签
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
    // 复审里的那个文件：3.5 KB 的 gzip，360 万个元素，在默认预算下能占到约 116 MiB；按 Java 的预算，分配之前就拒绝
    let file = gzip(&byte_list(3_600_000));
    assert!(file.len() < 8 * 1024, "压缩后只有 {} 字节", file.len());
    match parse_java(&file, &Limits::java()) {
        Err(Error::Nbt(e)) => assert!(matches!(e.kind, NbtErrorKind::TooMuchMemory(m) if m == Limits::JAVA_MAX_MEMORY), "{e}"),
        other => panic!("Java 的预算应该在分配之前就拒绝：{other:?}"),
    }
}

/// 原版结构 .nbt：满 48×48×48（结构方块能存的最大体积）每格一个方块，要能在 Java 的预算里解析
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
    // palette: 两种方块
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
    // blocks: 每格一个 {pos: [x, y, z], state}
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
    let s = parse_java(&gzip(&raw), &Limits::java()).unwrap_or_else(|e| panic!("满 48³ 的原版结构应该在 Java 的预算里：{e}"));
    assert_eq!(s.size, [48, 48, 48]);
    // 预算收到四分之一就放不下：这个数和真实文件是一个量级，不是随手给的大数
    let half = Limits { max_memory: Limits::JAVA_MAX_MEMORY / 4, ..Limits::java() };
    assert!(parse_java(&gzip(&raw), &half).is_err(), "预算的量级要和真实文件对得上");
}
