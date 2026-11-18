//! 真实样本的黄金测试。期望值由独立的 Python 参考实现算出（`tools/golden.py`，与本库不共享代码）。

use mc_structure::{Edition, Limits, parse};

fn load(name: &str) -> mc_structure::Structure {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    parse(&bytes, Some(name), &Limits::default()).unwrap()
}

#[test]
fn bedrock_format_version_1_two_layers() {
    let s = load("bedrock_v1_close.mcstructure");
    assert_eq!(s.edition, Edition::Bedrock);
    assert_eq!(s.format_version, 1);
    assert_eq!(s.size, [4, 17, 14]);
    assert_eq!(s.origin, Some([-12, 104, 180]));
    assert_eq!(s.palette.len(), 88);
    assert_eq!(s.block_count(), 685);
    assert_eq!(hex::encode(s.digest()), "d1a726fc7aa756bc25fac33a2e961f62e0bb0496efd1d9afcf8ff258669f26c5");
    let top: Vec<(String, u32)> = s.materials().into_iter().take(5).collect();
    assert_eq!(
        top,
        [
            ("minecraft:quartz_block".into(), 224),
            ("minecraft:sticky_piston".into(), 109),
            ("minecraft:redstone_wire".into(), 75),
            ("minecraft:observer".into(), 70),
            ("minecraft:sticky_piston_arm_collision".into(), 40),
        ]
    );
    assert_eq!(s.cell_key(&s.blocks[0]), "minecraft:observer[minecraft:facing_direction=east,powered_bit=0]");
    assert_eq!(s.blocks[0].pos, [0, 0, 0]);
    assert_eq!(s.block_entities.len(), 188);
}

/// 方块实体里记着导出时的世界坐标：原点 + 结构内坐标必须等于它。这同时验证了线性下标的展开顺序。
#[test]
fn bedrock_block_entity_world_coords_match_origin_plus_pos() {
    for name in ["bedrock_v1_close.mcstructure", "bedrock_v2_close.mcstructure"] {
        let s = load(name);
        let origin = s.origin.unwrap();
        let mut checked = 0;
        for be in &s.block_entities {
            let get = |k: &str| be.data.get(k).and_then(mc_structure::nbt::Tag::as_i32);
            if let (Some(x), Some(y), Some(z)) = (get("x"), get("y"), get("z")) {
                let pos = be.pos.map(|v| i32::try_from(v).unwrap());
                assert_eq!([x, y, z], [origin[0] + pos[0], origin[1] + pos[1], origin[2] + pos[2]], "{name} {:?}", be.pos);
                checked += 1;
            }
        }
        assert!(checked > 100, "{name} 只核对了 {checked} 个");
    }
}

#[test]
fn bedrock_format_version_2_int_array_layer() {
    let s = load("bedrock_v2_close.mcstructure");
    assert_eq!(s.format_version, 2);
    assert_eq!(s.size, [7, 12, 13]);
    assert_eq!(s.origin, Some([-59, 69, -126]));
    assert_eq!(s.palette.len(), 76);
    assert_eq!(s.block_count(), 623);
    assert_eq!(hex::encode(s.digest()), "d95aadded25e9a8f4f7b65f744749881f9130717b7c8d5a7ffccf108ecc4c19e");
    let top: Vec<(String, u32)> = s.materials().into_iter().take(5).collect();
    assert_eq!(
        top,
        [
            ("minecraft:quartz_block".into(), 196),
            ("minecraft:redstone_wire".into(), 86),
            ("minecraft:sticky_piston".into(), 74),
            ("minecraft:observer".into(), 58),
            ("minecraft:unpowered_repeater".into(), 47),
        ]
    );
    assert_eq!(s.cell_key(&s.blocks[0]), "minecraft:piston[facing_direction=4]");
    assert_eq!(s.block_entities.len(), 107);
}

#[test]
fn different_structures_have_different_digests_and_diff() {
    let a = load("bedrock_v1_close.mcstructure");
    let b = load("bedrock_v2_close.mcstructure");
    assert_ne!(a.digest(), b.digest());
    assert_eq!(a.diff(&a).moving, 0);
    assert!(a.diff(&b).moving > 0);
}

#[test]
fn truncated_real_file_fails_with_position() {
    let path = format!("{}/tests/fixtures/bedrock_v1_close.mcstructure", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(path).unwrap();
    let e = parse(&bytes[..bytes.len() / 2], Some("x.mcstructure"), &Limits::default()).unwrap_err();
    let msg = e.to_string();
    assert!(msg.contains("偏移"), "{msg}");
}
