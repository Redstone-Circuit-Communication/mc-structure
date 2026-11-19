//! Litematica 投影（`.litematic`）的测试。夹具与期望值由独立的 Python 参考实现生成、算出（`tools/litematic.py`，与本库不共享代码：
//! 它把整个 long 数组当成一个大整数按位切，本库是两个 long 之间移位拼接）。
//! 另外现场造投影：一个「一位一位放」的打包器（和库里的写法完全不同）覆盖各种调色板大小，再造各种坏文件。

// clippy.toml 的 allow-*-in-tests 只管 `#[test]` 函数和 `#[cfg(test)]` 模块；集成测试文件里的辅助函数（load、bytes_of、gzip……）
// 在 clippy 1.94 里不算「测试里」，会被 unwrap_used 等拦下。测试代码本来就该在出错时直接 panic，这里整个文件放行。
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

// ───────────────────────── 夹具：期望值全部来自 tools/litematic.py

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
        assert_eq!(lit.digest(), nbt.digest(), "{state}：调色板顺序、版本、负尺寸都不该影响摘要");
        assert_eq!(lit.materials(), nbt.materials());
    }
}

#[test]
fn door_golden_values() {
    // 关闭态：第 6 版，正尺寸，Position 不在原点
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
    // 开启态：第 7 版（带 SubVersion），三个轴的 Size 都是负数
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
    // 两态的差异：三个活塞变了状态、三个活塞头、红石线、侦测器
    let d = load("door_close.litematic").diff(&s);
    assert_eq!((d.moving, d.total), (8, 27));
}

#[test]
fn door_blocks_are_sorted_and_use_the_unified_palette() {
    let s = load("door_close.litematic");
    assert!(s.blocks.windows(2).all(|w| w[0].pos < w[1].pos), "按坐标严格升序");
    assert!(s.blocks.iter().all(|b| b.extra.is_none()));
    assert!(s.blocks.iter().all(|b| (b.state as usize) < s.palette.len()));
    // 左下角（0,0,0）是地面的石头
    assert_eq!(s.blocks[0].pos, [0, 0, 0]);
    assert_eq!(s.cell_key(&s.blocks[0]), "minecraft:stone");
    assert_eq!(s.origin, None);
}

#[test]
fn multi_region_with_overlap_negative_size_and_lying_metadata() {
    let s = load("multi_region.litematic");
    // Metadata 里写的是 TotalBlocks = 1、EnclosingSize = 1×1×1：不当真，以解码出来的为准
    assert_eq!(s.size, [8, 6, 4]);
    assert_eq!(s.block_count(), 29);
    assert_eq!(hex::encode(s.digest()), "719c8d597fb4ab1fed0a1cebf1a945ff435b321dc9ceb6a4fd886dcae085b2ab");
    assert_eq!(
        top(&s, 5),
        pairs(&[("minecraft:obsidian", 10), ("minecraft:stone", 9), ("minecraft:quartz_block", 6), ("minecraft:dirt", 2), ("minecraft:chest", 1)])
    );
    // wall 和 base 在 x = 3 重叠：后面的区域覆盖前面的，所以 base 的三块石头被石英盖掉了（石头只剩 9 块）
    let at = |p: [u32; 3]| s.blocks.iter().find(|b| b.pos == p).map(|b| s.cell_key(b));
    assert_eq!(at([3, 0, 1]).as_deref(), Some("minecraft:quartz_block"));
    assert_eq!(at([2, 0, 1]).as_deref(), Some("minecraft:stone"));
    // neg 区域（Size 为负）的最小角在 (6, 3, 2)，(0,0,0) 那一格是箱子
    assert_eq!(at([6, 3, 2]).as_deref(), Some("minecraft:chest[facing=north,type=single,waterlogged=false]"));
    // 同一个结构压平成一个原版结构，摘要一样
    let flat = load("multi_region_flat.nbt");
    assert_eq!(flat.digest(), s.digest());
    assert_eq!(flat.block_entities.len(), s.block_entities.len());
}

#[test]
fn multi_region_block_entities_and_entities_get_the_region_offset() {
    let s = load("multi_region.litematic");
    // 方块实体：位置加上区域的偏移（wall 的 (1,1,1) → (4,1,1)，neg 的 (0,0,0) → (6,3,2)），数据里区域内的 x、y、z 不再留
    let positions: Vec<[u32; 3]> = s.block_entities.iter().map(|b| b.pos).collect();
    assert_eq!(positions, [[4, 1, 1], [6, 3, 2]]);
    let chest = s.block_entities.iter().find(|b| b.pos == [6, 3, 2]).unwrap();
    assert_eq!(chest.data.get("id").and_then(Tag::as_str), Some("minecraft:chest"));
    assert!(chest.data.get("x").is_none() && chest.data.get("y").is_none() && chest.data.get("z").is_none());
    // 实体：Pos 是相对区域的浮点坐标，加上区域的偏移（neg 的最小角是 (6, 3, 2)）；其余的键原样
    assert_eq!(s.entities.len(), 1);
    let pos: Vec<f64> = s.entities[0].get("Pos").and_then(Tag::as_list).unwrap().iter().map(|t| if let Tag::Double(d) = t { *d } else { f64::NAN }).collect();
    assert_eq!(pos, [6.5, 3.0, 2.5]);
    assert_eq!(s.entities[0].get("id").and_then(Tag::as_str), Some("minecraft:armor_stand"));
    assert!(s.entities[0].get("Motion").and_then(Tag::as_list).is_some());
}

#[test]
fn real_litematica_export_sunspire_palace() {
    // Litematica 真导出的文件（站长提供，作者 yeaminist）：第 6 版，单区域 70×46×66，调色板 715 项（每个下标 10 位，跨 long 的边界），1825 个方块实体
    let s = load("sunspire_palace.litematic");
    assert_eq!((s.format, s.format_version, s.size), (Format::Litematic, 5023, [70, 46, 66]));
    // 和 Litematica 自己写在 Metadata 里的 TotalBlocks（21459）、EnclosingSize（70×46×66）对得上
    assert_eq!(s.block_count(), 21459);
    assert_eq!(hex::encode(s.digest()), "cedd169a7f609de0c09d04f033c77e83aeef528ec05cc84e4b1fe04939ef4cdf");
    let want = pairs(&[
        ("minecraft:birch_planks", 2707),
        ("minecraft:sandstone", 2180),
        ("minecraft:smooth_sandstone", 1788),
        ("minecraft:red_sandstone", 912),
        ("minecraft:sandstone_wall", 758),
    ]);
    assert_eq!(top(&s, 5), want);
    assert_eq!((s.block_entities.len(), s.entities.len()), (1825, 0));
    // 格子顺序对不对：方块实体的坐标是文件里另外记的。顺序对，它们所在的格子解出来都是带方块实体的方块（架子、告示牌、木桶、箱子……）；
    // 顺序错了（比如 x、y 互换），1825 个里有 1629 个会落在空气上
    let occupied: std::collections::HashSet<[u32; 3]> = s.blocks.iter().map(|b| b.pos).collect();
    let on_air = s.block_entities.iter().filter(|be| !occupied.contains(&be.pos)).count();
    assert_eq!(on_air, 0, "方块实体落在空气上：格子顺序不对");
}

#[test]
fn palettes_that_cross_long_boundaries() {
    // 17 项 → 每个下标 5 位（105 个下标 = 525 位，跨 long 的边界很多次）
    let s = load("palette17.litematic");
    assert_eq!((s.size, s.block_count()), ([7, 5, 3], 98));
    assert_eq!(hex::encode(s.digest()), "ddea78ff5bd9cb157fbe743d15836941045fd5434505b55067d5d28a043bcb8c");
    // 正好 64 项（2 的 6 次方）→ 6 位，64 不是 6 的倍数，同样跨边界
    let s = load("palette64.litematic");
    assert_eq!((s.size, s.block_count()), ([5, 4, 4], 79));
    assert_eq!(hex::encode(s.digest()), "be74d993a9eb2d66a3a8f6928ffbb1fcd7bd452be5ec1d0206bc30aa3304afc4");
}

#[test]
fn parsed_by_content_not_by_extension() {
    // 把投影改名成 .nbt：照内容解析，格式以内容为准
    let lit = bytes_of("door_close.litematic");
    let s = parse(&lit, Some("door.nbt"), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::Litematic);
    // 反过来：原版结构改名成 .litematic
    let nbt = bytes_of("door_close.nbt");
    let s = parse(&nbt, Some("door.litematic"), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::JavaStructure);
    // 没有文件名也行（gzip 一定是 Java）
    assert_eq!(parse(&lit, None, &Limits::default()).unwrap().format, Format::Litematic);
}

#[test]
fn uncompressed_litematic_needs_the_extension_to_be_recognised() {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(bytes_of("door_close.litematic").as_slice()).read_to_end(&mut raw).unwrap();
    let s = parse(&raw, Some("door.litematic"), &Limits::default()).unwrap();
    assert_eq!(s.format, Format::Litematic);
    assert_eq!(hex::encode(s.digest()), "fef504d3bec3fd5d6db0bc69bc4318b52243ecf41a8d139d7f0b5076fdfbad33");
    // 未压缩又没有后缀：看不出版本，不猜
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

// ───────────────────────── 现场造投影

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

/// 每个下标占的位数：`max(2, ceil(log2(n)))`，用循环算（不用库里的前导零写法）。
fn bits_of(palette_len: usize) -> u32 {
    let mut bits = 2;
    while (1usize << bits) < palette_len {
        bits += 1;
    }
    bits
}

/// 紧密打包：下标 i 从第 i·bits 位开始，跨 long 的边界接着放，第一个 long 的最低位是第 0 位。
/// 一位一位地放，不用任何移位拼接——和库里的写法完全不同。
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
        // 期望：下标不是 0（空气）的格子；x 最快、然后 z、然后 y
        let mut want: Vec<([u32; 3], String)> = Vec::new();
        for (i, v) in indices.iter().enumerate() {
            if *v != 0 {
                let (x, z, y) = (i % nx, (i / nx) % nz, i / (nx * nz));
                want.push(([x as u32, y as u32, z as u32], format!("minecraft:b{v}")));
            }
        }
        want.sort();
        // 正尺寸，和 Size 全为负、Position 在最大角的同一个区域，读出来要一样
        for (position, size) in [([0, 0, 0], [6, 5, 4]), ([9, 8, 7], [-6, -5, -4])] {
            let st = parse_default(&one(region(position, size, &refs, &indices))).unwrap();
            assert_eq!(st.size, [6, 5, 4], "调色板 {n} 项");
            let got: Vec<([u32; 3], String)> = st.cells().collect();
            assert_eq!(got, want, "调色板 {n} 项，Size {size:?}");
        }
    }
}

#[test]
fn regions_merge_and_air_never_overrides() {
    // a：1×1×1 的石头；b：同一格，空气 → 石头还在。c：同一格，泥土 → 盖掉石头
    let air_stone = ["minecraft:air", "minecraft:stone"];
    let air_dirt = ["minecraft:air", "minecraft:dirt"];
    let a = region([0, 0, 0], [1, 1, 1], &air_stone, &[1]);
    let b = region([0, 0, 0], [1, 1, 1], &air_dirt, &[0]);
    let c = region([0, 0, 0], [1, 1, 1], &air_dirt, &[1]);
    let st = parse_default(&file(vec![("a".into(), a.clone()), ("b".into(), b.clone())])).unwrap();
    assert_eq!(st.cells().collect::<Vec<_>>(), [([0, 0, 0], "minecraft:stone".to_owned())]);
    let st = parse_default(&file(vec![("a".into(), a), ("b".into(), b), ("c".into(), c.clone())])).unwrap();
    assert_eq!(st.cells().collect::<Vec<_>>(), [([0, 0, 0], "minecraft:dirt".to_owned())]);
    // 三个区域的调色板合并：air 在三个里都有、dirt 在 b 和 c 里都有，统一调色板里各只留一份
    assert_eq!(st.palette.len(), 3, "air、stone、dirt 各一份");
    assert_eq!(st.palette.iter().filter(|p| p.name == "minecraft:dirt").count(), 1);
    // 顺序倒过来：泥土在前、石头在后，石头盖掉泥土
    let a2 = region([0, 0, 0], [1, 1, 1], &air_stone, &[1]);
    let st = parse_default(&file(vec![("c".into(), c), ("a".into(), a2)])).unwrap();
    assert_eq!(st.cells().collect::<Vec<_>>(), [([0, 0, 0], "minecraft:stone".to_owned())]);
}

#[test]
fn regions_far_apart_take_the_bounding_box() {
    // 两个 1×1×1 的区域，一个在原点、一个在 (5, -2, 3)：范围是 6×3×4，最小角移到 0
    let pal = ["minecraft:air", "minecraft:stone"];
    let a = region([0, 0, 0], [1, 1, 1], &pal, &[1]);
    let b = region([5, -2, 3], [1, 1, 1], &pal, &[1]);
    let st = parse_default(&file(vec![("a".into(), a), ("b".into(), b)])).unwrap();
    assert_eq!(st.size, [6, 3, 4]);
    let cells: Vec<[u32; 3]> = st.cells().map(|(p, _)| p).collect();
    assert_eq!(cells, [[0, 2, 0], [5, 0, 3]]);
}

// ───────────────────────── 坏文件

fn expect_invalid(bytes: &[u8], needle: &str) {
    let e = parse_default(bytes).unwrap_err();
    assert!(matches!(e, Error::Invalid { .. }), "{e}");
    assert!(e.to_string().contains(needle), "{e}");
}

#[test]
fn short_block_states_is_an_error_but_extra_longs_are_ignored() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let good = pack(&[1u64; 24], 2);
    // 少了就是缺数据
    let mut short = good.clone();
    short.pop();
    for longs in [short, vec![]] {
        let bytes = one(region_with([0, 0, 0], [4, 3, 2], palette_tag(&pal), Tag::LongArray(longs)));
        expect_invalid(&bytes, "Regions.r.BlockStates");
    }
    // 多出来的 long 不管：Litematica 写的正好是这么多，别的工具可能补零（站长以前的项目也是这样读的）
    let mut extra = good;
    extra.push(0);
    let st = parse_default(&one(region_with([0, 0, 0], [4, 3, 2], palette_tag(&pal), Tag::LongArray(extra)))).unwrap();
    assert_eq!(st.block_count(), 24);
}

#[test]
fn block_entities_key_is_accepted_too() {
    // 方块实体的键名一般是 TileEntities，也见过 BlockEntities
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
    // 3 项的调色板（2 位，能写出下标 3），但有一格写了 3
    let pal = ["minecraft:air", "minecraft:stone", "minecraft:dirt"];
    let bytes = one(region_with([0, 0, 0], [2, 1, 1], palette_tag(&pal), Tag::LongArray(pack(&[1, 3], 2))));
    expect_invalid(&bytes, "越界");
}

#[test]
fn missing_or_wrong_typed_parts_are_errors() {
    let pal = ["minecraft:air", "minecraft:stone"];
    let long = || Tag::LongArray(pack(&[1], 2));
    // 没有调色板、调色板是空的、BlockStates 不是 LongArray、缺 Position、缺 Size
    let no_palette = compound(vec![("Position", xyz(0, 0, 0)), ("Size", xyz(1, 1, 1)), ("BlockStates", long())]);
    expect_invalid(&one(no_palette), "Regions.r.BlockStatePalette");
    expect_invalid(&one(region_with([0, 0, 0], [1, 1, 1], Tag::List(vec![]), long())), "调色板是空的");
    expect_invalid(&one(region_with([0, 0, 0], [1, 1, 1], palette_tag(&pal), Tag::IntArray(vec![1]))), "LongArray");
    let no_pos = compound(vec![("Size", xyz(1, 1, 1)), ("BlockStatePalette", palette_tag(&pal)), ("BlockStates", long())]);
    expect_invalid(&one(no_pos), "Regions.r.Position");
    let no_size = compound(vec![("Position", xyz(0, 0, 0)), ("BlockStatePalette", palette_tag(&pal)), ("BlockStates", long())]);
    expect_invalid(&one(no_size), "Regions.r.Size");
    // 区域本身不是复合标签；一个区域都没有
    expect_invalid(&file(vec![("r".into(), Tag::Int(1))]), "Regions.r");
    expect_invalid(&file(vec![]), "没有任何区域");
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
    // 单轴超过上限、尺寸为 0（BlockStates 是空的也不会去分配或解码）
    too_large(one(region_with([0, 0, 0], [20_000, 1, 1], palette_tag(&pal), Tag::LongArray(vec![]))), &limits);
    too_large(one(region_with([0, 0, 0], [0, 1, 1], palette_tag(&pal), Tag::LongArray(vec![]))), &limits);
    too_large(one(region_with([0, 0, 0], [i32::MIN, 1, 1], palette_tag(&pal), Tag::LongArray(vec![]))), &limits);
    // 体积超过元素上限
    let small = Limits { max_elements: 100, ..Limits::default() };
    too_large(one(region_with([0, 0, 0], [10, 10, 2], palette_tag(&pal), Tag::LongArray(vec![]))), &small);
    // 每个区域都不超，加起来超（6×6×2 = 72，两个 144 > 100）
    let r = || region_with([0, 0, 0], [6, 6, 2], palette_tag(&pal), Tag::LongArray(pack(&[1u64; 72], 2)));
    too_large(file(vec![("a".into(), r()), ("b".into(), r())]), &small);
    // 两个很远的区域：各自都小，范围合起来超过单轴上限
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
    // 有 Regions 但不是复合标签，也没有原版结构的任何键：不认识
    let root = Compound(vec![("Regions".into(), Tag::List(vec![])), ("MinecraftDataVersion".into(), Tag::Int(1))]);
    assert!(matches!(parse_default(&file_from(root)), Err(Error::UnknownFormat)));
    // Sponge .schem 的根
    let schem = Compound(vec![("Version".into(), Tag::Int(2)), ("Width".into(), Tag::Short(1)), ("Palette".into(), Tag::Compound(Compound(vec![])))]);
    assert!(matches!(parse_default(&file_from(schem)), Err(Error::UnknownFormat)));
}

#[test]
fn missing_data_version_is_tolerated_as_zero() {
    // 老版本的投影没有 MinecraftDataVersion：照样是投影，format_version 记 0
    let pal = ["minecraft:air", "minecraft:stone"];
    let root = Compound(vec![("Regions".into(), Tag::Compound(Compound(vec![("r".into(), region([0, 0, 0], [1, 1, 1], &pal, &[1]))])))]);
    let st = parse_default(&file_from(root)).unwrap();
    assert_eq!((st.format, st.format_version, st.block_count()), (Format::Litematic, 0, 1));
}
