//! 统一的结构模型与它上面的计算：方块数、材料表、内容摘要、两态差异。

use std::collections::HashMap;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::nbt::Compound;

/// 版本。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edition {
    /// Java 版
    Java,
    /// 基岩版
    Bedrock,
}

impl Edition {
    /// `java` / `bedrock`
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Java => "java",
            Self::Bedrock => "bedrock",
        }
    }
}

/// 文件格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// Java 版原版结构（`.nbt`）
    JavaStructure,
    /// Java 版 Litematica 投影（`.litematic`）
    Litematic,
    /// 基岩版 `.mcstructure`
    McStructure,
}

impl Format {
    /// `nbt` / `litematic` / `mcstructure`（也就是这种文件的扩展名）
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JavaStructure => "nbt",
            Self::Litematic => "litematic",
            Self::McStructure => "mcstructure",
        }
    }
}

/// 方块状态的值。基岩的状态有字节、整数、字符串三种；Java 的属性一律是字符串。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateValue {
    /// 字节（基岩里多是开关：`open_bit = 0`）
    Byte(i8),
    /// 整数
    Int(i32),
    /// 字符串
    String(String),
}

impl fmt::Display for StateValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Byte(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::String(v) => f.write_str(v),
        }
    }
}

/// 调色板里的一项：方块 id 与按键排序的状态。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockState {
    /// 方块 id，保持原版本的写法（`minecraft:sticky_piston`）
    pub name: String,
    /// 状态，按键排序
    pub states: Vec<(String, StateValue)>,
}

impl BlockState {
    /// 构造并把状态按键排序（同一状态只有一种写法）。
    #[must_use]
    pub fn new(name: impl Into<String>, mut states: Vec<(String, StateValue)>) -> Self {
        // 按 &str 比较：键是借来的，新版 clippy 的 unnecessary_sort_by 不会建议一个要搬走 String 的 sort_by_key
        states.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Self { name: name.into(), states }
    }

    /// 空气与结构空位：不算方块。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:structure_void")
    }

    /// 规范写法：`name` 或 `name[k=v,k=v]`（摘要与两态比较都用它）。
    #[must_use]
    pub fn key(&self) -> String {
        if self.states.is_empty() {
            return self.name.clone();
        }
        let inner: Vec<String> = self.states.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}[{}]", self.name, inner.join(","))
    }
}

/// 一个非空的格子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// 结构内的坐标（0 起，`< size`）
    pub pos: [u32; 3],
    /// 调色板下标
    pub state: u32,
    /// 第二层（基岩的含水层）：这一格同时含有的方块，如水
    pub extra: Option<u32>,
}

/// 方块实体（活塞臂、漏斗、比较器……）。
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEntity {
    /// 结构内的坐标
    pub pos: [u32; 3],
    /// 原样的 NBT
    pub data: Compound,
}

/// 两态差异（`moving_blocks`）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormDiff {
    /// 不同的格子数：一边有方块另一边没有，或方块（含状态）不同
    pub moving: u32,
    /// 任意一边非空的格子数
    pub total: u32,
}

impl FormDiff {
    /// 不同的比例（0..=1）。
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.total == 0 { 0.0 } else { f64::from(self.moving) / f64::from(self.total) }
    }
}

/// 一个结构。
#[derive(Clone, Debug, PartialEq)]
pub struct Structure {
    /// 版本
    pub edition: Edition,
    /// 格式
    pub format: Format,
    /// Java 为 `DataVersion`；基岩为 `format_version`
    pub format_version: i32,
    /// `[x, y, z]`
    pub size: [u32; 3],
    /// 基岩的 `structure_world_origin`（导出时的世界坐标）；Java 没有
    pub origin: Option<[i32; 3]>,
    /// 调色板（原样，含空气）
    pub palette: Vec<BlockState>,
    /// 非空格子，按坐标排序
    pub blocks: Vec<Block>,
    /// 方块实体
    pub block_entities: Vec<BlockEntity>,
    /// 实体（原样的 NBT）
    pub entities: Vec<Compound>,
}

impl Structure {
    /// 非空方块数。
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// 一格的规范写法：主方块，含水层用 `+` 接在后面。
    #[must_use]
    pub fn cell_key(&self, b: &Block) -> String {
        let key = |i: u32| self.palette.get(i as usize).map_or_else(|| format!("#{i}"), BlockState::key);
        match b.extra {
            Some(e) => format!("{}+{}", key(b.state), key(e)),
            None => key(b.state),
        }
    }

    /// 内容摘要：尺寸 + 每个非空格子的坐标与规范写法（按坐标排序）。
    /// 两个文件摘要相同就是同一个结构，与文件里调色板的顺序、压缩方式无关。
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        let [x, y, z] = self.size;
        h.update(format!("{x}x{y}x{z}\n").as_bytes());
        for b in &self.blocks {
            let [bx, by, bz] = b.pos;
            h.update(format!("{bx},{by},{bz} {}\n", self.cell_key(b)).as_bytes());
        }
        h.finalize().into()
    }

    /// 材料表：方块 id → 数量（只数主方块），按数量降序、同数按 id。
    #[must_use]
    pub fn materials(&self) -> Vec<(String, u32)> {
        let mut m: HashMap<&str, u32> = HashMap::new();
        for b in &self.blocks {
            if let Some(s) = self.palette.get(b.state as usize) {
                *m.entry(s.name.as_str()).or_default() += 1;
            }
        }
        let mut v: Vec<(String, u32)> = m.into_iter().map(|(k, n)| (k.to_owned(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// 和另一态比较（同一版本的开、关两态）。只比方块，不比实体。
    #[must_use]
    pub fn diff(&self, other: &Self) -> FormDiff {
        let left: HashMap<[u32; 3], String> = self.blocks.iter().map(|b| (b.pos, self.cell_key(b))).collect();
        let right: HashMap<[u32; 3], String> = other.blocks.iter().map(|b| (b.pos, other.cell_key(b))).collect();
        let mut moving = 0u32;
        let mut total = 0u32;
        for (pos, k) in &left {
            total += 1;
            if right.get(pos) != Some(k) {
                moving += 1;
            }
        }
        for pos in right.keys() {
            if !left.contains_key(pos) {
                total += 1;
                moving += 1;
            }
        }
        FormDiff { moving, total }
    }

    /// 方块的规范写法列表（坐标 + 写法），给需要逐格比较的调用方。
    pub fn cells(&self) -> impl Iterator<Item = ([u32; 3], String)> + '_ {
        self.blocks.iter().map(|b| (b.pos, self.cell_key(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(name: &str, states: &[(&str, StateValue)]) -> BlockState {
        BlockState::new(name, states.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect())
    }

    fn s(palette: Vec<BlockState>, blocks: &[([u32; 3], u32)]) -> Structure {
        Structure {
            edition: Edition::Bedrock,
            format: Format::McStructure,
            format_version: 1,
            size: [3, 1, 1],
            origin: None,
            palette,
            blocks: blocks.iter().map(|(p, i)| Block { pos: *p, state: *i, extra: None }).collect(),
            block_entities: vec![],
            entities: vec![],
        }
    }

    #[test]
    fn key_is_sorted_and_typed() {
        let b = st("minecraft:observer", &[("powered_bit", StateValue::Byte(0)), ("minecraft:facing_direction", StateValue::String("east".into()))]);
        assert_eq!(b.key(), "minecraft:observer[minecraft:facing_direction=east,powered_bit=0]");
        assert_eq!(st("minecraft:stone", &[]).key(), "minecraft:stone");
    }

    #[test]
    fn digest_ignores_palette_order() {
        let a = s(vec![st("minecraft:stone", &[]), st("minecraft:dirt", &[])], &[([0, 0, 0], 0), ([1, 0, 0], 1)]);
        let b = s(vec![st("minecraft:dirt", &[]), st("minecraft:stone", &[])], &[([0, 0, 0], 1), ([1, 0, 0], 0)]);
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn diff_counts_state_changes_and_presence() {
        let p = vec![
            st("minecraft:piston", &[("extended", StateValue::Byte(0))]),
            st("minecraft:piston", &[("extended", StateValue::Byte(1))]),
            st("minecraft:piston_arm_collision", &[]),
        ];
        let close = s(p.clone(), &[([0, 0, 0], 0)]);
        let open = s(p, &[([0, 0, 0], 1), ([1, 0, 0], 2)]);
        let d = close.diff(&open);
        assert_eq!((d.moving, d.total), (2, 2));
        assert!((d.ratio() - 1.0).abs() < f64::EPSILON);
        assert_eq!(close.diff(&close).moving, 0);
    }
}
