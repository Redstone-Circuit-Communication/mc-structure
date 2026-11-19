//! The common structure model and what is computed from it: block count, material list, content digest and the difference
//! between two states.

use std::collections::HashMap;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::nbt::Compound;

/// Game edition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edition {
    /// Java Edition
    Java,
    /// Bedrock Edition
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

/// File format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// Java vanilla structure (`.nbt`)
    JavaStructure,
    /// Java Litematica schematic (`.litematic`)
    Litematic,
    /// Bedrock `.mcstructure`
    McStructure,
}

impl Format {
    /// `nbt` / `litematic` / `mcstructure` (the file extension of the format)
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::JavaStructure => "nbt",
            Self::Litematic => "litematic",
            Self::McStructure => "mcstructure",
        }
    }
}

/// A block state value. Bedrock states are bytes, integers or strings; Java properties are always strings.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateValue {
    /// Byte (in Bedrock usually a flag, such as `open_bit = 0`)
    Byte(i8),
    /// Integer
    Int(i32),
    /// String
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

/// A palette entry: the block id and its states sorted by key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockState {
    /// Block id as the edition spells it (`minecraft:sticky_piston`)
    pub name: String,
    /// States, sorted by key
    pub states: Vec<(String, StateValue)>,
}

impl BlockState {
    /// Builds an entry with its states sorted by key, so that each state has exactly one spelling.
    #[must_use]
    pub fn new(name: impl Into<String>, mut states: Vec<(String, StateValue)>) -> Self {
        // Compare as &str: the keys are borrowed, and this keeps newer clippy (unnecessary_sort_by) from suggesting a
        // sort_by_key that would have to clone each String
        states.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Self { name: name.into(), states }
    }

    /// Air and structure voids, which do not count as blocks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:structure_void")
    }

    /// Canonical spelling, `name` or `name[k=v,k=v]`, used by the digest and by the state comparison.
    #[must_use]
    pub fn key(&self) -> String {
        if self.states.is_empty() {
            return self.name.clone();
        }
        let inner: Vec<String> = self.states.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}[{}]", self.name, inner.join(","))
    }
}

/// A non-empty cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    /// Position inside the structure (zero-based, `< size`)
    pub pos: [u32; 3],
    /// Palette index
    pub state: u32,
    /// Second layer (Bedrock's waterlogging layer): another block in the same cell, such as water
    pub extra: Option<u32>,
}

/// A block entity (moving piston, hopper, comparator, …).
#[derive(Clone, Debug, PartialEq)]
pub struct BlockEntity {
    /// Position inside the structure
    pub pos: [u32; 3],
    /// The NBT as stored
    pub data: Compound,
}

/// Difference between two states (`moving_blocks`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormDiff {
    /// Cells that differ: a block on one side and none on the other, or a different block (states included)
    pub moving: u32,
    /// Cells that are non-empty on at least one side
    pub total: u32,
}

impl FormDiff {
    /// Share of differing cells (0..=1).
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.total == 0 { 0.0 } else { f64::from(self.moving) / f64::from(self.total) }
    }
}

/// A structure.
#[derive(Clone, Debug, PartialEq)]
pub struct Structure {
    /// Edition
    pub edition: Edition,
    /// Format
    pub format: Format,
    /// `DataVersion` for Java, `format_version` for Bedrock
    pub format_version: i32,
    /// `[x, y, z]`
    pub size: [u32; 3],
    /// Bedrock's `structure_world_origin` (world position at the time of export); Java has none
    pub origin: Option<[i32; 3]>,
    /// Palette as stored, including air
    pub palette: Vec<BlockState>,
    /// Non-empty cells, sorted by position
    pub blocks: Vec<Block>,
    /// Block entities
    pub block_entities: Vec<BlockEntity>,
    /// Entities (NBT as stored)
    pub entities: Vec<Compound>,
}

impl Structure {
    /// Number of non-air blocks.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Canonical spelling of a cell: the main block, followed by the waterlogging layer after a `+`.
    #[must_use]
    pub fn cell_key(&self, b: &Block) -> String {
        let key = |i: u32| self.palette.get(i as usize).map_or_else(|| format!("#{i}"), BlockState::key);
        match b.extra {
            Some(e) => format!("{}+{}", key(b.state), key(e)),
            None => key(b.state),
        }
    }

    /// Content digest: the size plus the position and canonical spelling of every non-empty cell (sorted by position).
    /// Two files with the same digest hold the same structure, regardless of palette order or compression.
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

    /// Material list: block id → count (main blocks only), by count descending, then by id.
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

    /// Compares with the other state (the open and closed states of the same build). Compares blocks only, not entities.
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

    /// Canonical spelling of each block (position + spelling), for callers that compare cell by cell.
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
