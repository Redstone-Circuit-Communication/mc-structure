//! Minecraft 结构文件解析。
//!
//! | 版本 | 格式 | 文件 |
//! | --- | --- | --- |
//! | 基岩版 | `.mcstructure`（未压缩的小端 NBT；`format_version` 1、2） | 结构方块「导出」 |
//! | Java 版 | 原版结构（gzip 压缩的大端 NBT） | 结构方块「保存」到 `generated/<ns>/structures/*.nbt` |
//! | Java 版 | Litematica 投影 `.litematic`（gzip 压缩的大端 NBT；一个或多个区域，位压缩的方块） | Litematica 模组保存的投影 |
//!
//! 三种格式解析到同一个 [`Structure`]：调色板 + 非空方块（按坐标排序）+ 方块实体 + 实体。
//! 同一个结构存成 `.nbt` 或 `.litematic`，[`Structure::digest`]、[`Structure::block_count`]、[`Structure::materials`] 都一样。
//! 方块名保持各自版本的原样（基岩是基岩的 id，Java 是 Java 的 id），**本库不做版本之间的方块映射**。
//!
//! 输入不可信：所有长度先对照剩余字节与元素预算再分配，嵌套深度、元素总数、解压大小、各轴尺寸都有上限
//! （[`Limits`]）；错误带字节偏移与 NBT 路径（如 `structure.block_indices[0]`）。解析路径上不 panic。
//!
//! ```no_run
//! let bytes = std::fs::read("door_close.mcstructure").unwrap();
//! let s = mc_structure::parse(&bytes, Some("door_close.mcstructure"), &mc_structure::Limits::default()).unwrap();
//! println!("{:?} {:?} 共 {} 个方块", s.edition, s.size, s.block_count());
//! for (name, n) in s.materials().iter().take(5) {
//!     println!("{name} × {n}");
//! }
//! ```

mod bedrock;
mod error;
mod java;
mod litematic;
pub mod nbt;
mod structure;

pub use error::Error;
pub use structure::{Block, BlockEntity, BlockState, Edition, FormDiff, Format, StateValue, Structure};

/// 资源上限。默认值与红石阁旧后端一致。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// NBT 嵌套深度
    pub max_depth: u32,
    /// 元素总数（每个标签算 1，数组与列表按长度算），也是体积 x·y·z 的上限
    pub max_elements: u64,
    /// 解压后的字节数
    pub max_inflated: u64,
    /// 单轴尺寸
    pub max_axis: u32,
    /// 解析出来的标签在内存里最多占多少字节（估算：每个标签本身 32 字节，加上字符串、数组、列表、复合标签在堆上的部分），**分配之前**核对。
    /// 按字节记，不按个数：线上只占 1 字节的列表元素，在内存里是一个 32 字节的标签，再加上 gzip，几 KB 的文件能变成上百 MB。
    pub max_memory: u64,
}

impl Default for Limits {
    fn default() -> Self {
        // 112 MiB：装得下本站上限的旧版基岩文件（format_version 1 的方块下标是 List<Int>：64×384×64 两层，约 96 MiB）
        Self { max_depth: 64, max_elements: 8_000_000, max_inflated: 64 * 1024 * 1024, max_axis: 10_000, max_memory: 112 * 1024 * 1024 }
    }
}

impl Limits {
    /// Java 版两种格式的内存预算（评审 M1）。112 MiB 的默认值是给旧版基岩文件的（方块下标是 `List<Int>`，每个元素在内存里是一个标签）；
    /// Java 用不着：投影的方块是紧凑的 `LongArray`，原版结构 `.nbt` 每个方块约 250 字节，满 48×48×48 也只要约 27 MiB（`tests/memory.rs` 量着）。
    /// 压缩过的文件只可能是 Java，几 KB 的 gzip 能让解析占到的内存从约 116 MiB 降到这个数。
    pub const JAVA_MAX_MEMORY: u64 = 48 * 1024 * 1024;

    /// Java 版（`.nbt`、`.litematic`）的默认上限：只有内存预算比 [`Limits::default`] 小。
    #[must_use]
    pub fn java() -> Self {
        Self { max_memory: Self::JAVA_MAX_MEMORY, ..Self::default() }
    }

    /// 按版本取默认上限：Java 用 [`Limits::java`]，基岩用 [`Limits::default`]。
    #[must_use]
    pub fn for_edition(edition: Edition) -> Self {
        match edition {
            Edition::Java => Self::java(),
            Edition::Bedrock => Self::default(),
        }
    }
}

/// 按内容（魔数）和文件名判断版本。
///
/// gzip / zlib 压缩的一定是 Java；未压缩的根复合标签两边长得一样（`0a 00 00`），只能靠文件名区分。
/// 判断不了返回 `None`，由调用方要求用户说明——不猜（把任意文件交给某个解析器去试只会得到难懂的错误）。
#[must_use]
pub fn detect(bytes: &[u8], filename: Option<&str>) -> Option<Edition> {
    if is_compressed(bytes) {
        return Some(Edition::Java);
    }
    let ext = filename.and_then(|f| f.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("mcstructure") => Some(Edition::Bedrock),
        Some("nbt" | "litematic") => Some(Edition::Java),
        _ => None,
    }
}

pub(crate) fn is_compressed(bytes: &[u8]) -> bool {
    match bytes {
        [0x1f, 0x8b, ..] => true,
        [0x78, b, ..] => (u16::from(0x78_u8) << 8 | u16::from(*b)) % 31 == 0,
        _ => false,
    }
}

/// 识别并解析。识别不了返回 [`Error::UnknownFormat`]。
pub fn parse(bytes: &[u8], filename: Option<&str>, limits: &Limits) -> Result<Structure, Error> {
    match detect(bytes, filename) {
        Some(Edition::Bedrock) => parse_bedrock(bytes, limits),
        Some(Edition::Java) => parse_java(bytes, limits),
        None => Err(Error::UnknownFormat),
    }
}

/// 解析基岩版 `.mcstructure`。
pub fn parse_bedrock(bytes: &[u8], limits: &Limits) -> Result<Structure, Error> {
    bedrock::parse(bytes, limits)
}

/// 解析 Java 版的结构文件：原版结构 `.nbt` 或 Litematica 投影 `.litematic`（gzip、zlib 或未压缩都接受）。
/// 按**内容**分流，不按扩展名：根上有 `Regions` 的是投影，有 `size`、`blocks`、`palette`、`palettes` 之一的是原版结构，
/// 都不是（比如 `.schem`）返回 [`Error::UnknownFormat`]。扩展名和内容对不上（把投影改名成 `.nbt`）照内容解析，[`Structure::format`] 以内容为准。
pub fn parse_java(bytes: &[u8], limits: &Limits) -> Result<Structure, Error> {
    java::parse(bytes, limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_by_magic_and_name() {
        assert_eq!(detect(&[0x1f, 0x8b, 8], None), Some(Edition::Java));
        assert_eq!(detect(&[0x78, 0x9c, 0], None), Some(Edition::Java));
        assert_eq!(detect(&[0x0a, 0, 0], Some("a.MCSTRUCTURE")), Some(Edition::Bedrock));
        assert_eq!(detect(&[0x0a, 0, 0], Some("a.nbt")), Some(Edition::Java));
        assert_eq!(detect(&[0x0a, 0, 0], Some("a.LITEMATIC")), Some(Edition::Java));
        assert_eq!(detect(&[0x0a, 0, 0], None), None);
        assert_eq!(detect(b"hello", Some("x.txt")), None);
    }
}
