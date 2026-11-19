//! Parsing of Minecraft structure files.
//!
//! | Edition | Format | Produced by |
//! | --- | --- | --- |
//! | Bedrock | `.mcstructure` (uncompressed little-endian NBT; `format_version` 1 and 2) | The structure block's "Export" |
//! | Java | Vanilla structure (gzip-compressed big-endian NBT) | The structure block's "Save", to `generated/<ns>/structures/*.nbt` |
//! | Java | Litematica schematic `.litematic` (gzip-compressed big-endian NBT; one or more regions, bit-packed blocks) | The Litematica mod |
//!
//! All three formats parse into the same [`Structure`]: a palette, the non-air blocks (sorted by position), block entities and entities.
//! The same structure saved as `.nbt` or as `.litematic` gives the same [`Structure::digest`], [`Structure::block_count`] and [`Structure::materials`].
//! Block names stay as each edition spells them (Bedrock ids for Bedrock, Java ids for Java); **this crate does not map blocks between editions**.
//!
//! Input is treated as untrusted: every length is checked against the remaining bytes and the element budget before anything is
//! allocated, and nesting depth, element count, decompressed size and the size of each axis are capped ([`Limits`]). Errors carry
//! the byte offset and the NBT path (such as `structure.block_indices[0]`). Nothing on the parse path panics.
//!
//! ```no_run
//! let bytes = std::fs::read("door_close.mcstructure").unwrap();
//! let s = mc_structure::parse(&bytes, Some("door_close.mcstructure"), &mc_structure::Limits::default()).unwrap();
//! println!("{:?} {:?}, {} blocks", s.edition, s.size, s.block_count());
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

/// Resource limits. The defaults accept everything a structure block can save, including large Bedrock files in the old format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// NBT nesting depth.
    pub max_depth: u32,
    /// Total element count (each tag counts 1, arrays and lists count their length); also the cap on the volume x·y·z.
    pub max_elements: u64,
    /// Size after decompression, in bytes.
    pub max_inflated: u64,
    /// Size along any one axis.
    pub max_axis: u32,
    /// How many bytes the parsed tags may take in memory (estimated as 32 bytes per tag plus the heap part of strings, arrays,
    /// lists and compounds), checked **before** allocating.
    /// Counted in bytes rather than elements: a list element that takes 1 byte in the file is a 32-byte tag in memory, so
    /// together with gzip a file of a few KB could otherwise grow to hundreds of MB.
    pub max_memory: u64,
}

impl Default for Limits {
    fn default() -> Self {
        // 112 MiB: fits the largest Bedrock file in the old format (format_version 1 stores block indices as a List<Int>;
        // two layers of 64×384×64 take about 96 MiB)
        Self { max_depth: 64, max_elements: 8_000_000, max_inflated: 64 * 1024 * 1024, max_axis: 10_000, max_memory: 112 * 1024 * 1024 }
    }
}

impl Limits {
    /// Memory budget for the two Java formats. The 112 MiB default exists for old Bedrock files (block indices are a `List<Int>`,
    /// with one tag in memory per element). Java needs far less: a Litematica schematic stores its blocks in a compact `LongArray`,
    /// and a vanilla `.nbt` takes about 250 bytes per block, about 27 MiB for a full 48×48×48 (measured in `tests/memory.rs`).
    /// Only Java files can be compressed, so this lowers the memory that a few KB of gzip can make the parser use from about
    /// 116 MiB to this figure.
    pub const JAVA_MAX_MEMORY: u64 = 48 * 1024 * 1024;

    /// Default limits for Java (`.nbt`, `.litematic`): the same as [`Limits::default`] except for a smaller memory budget.
    #[must_use]
    pub fn java() -> Self {
        Self { max_memory: Self::JAVA_MAX_MEMORY, ..Self::default() }
    }

    /// Default limits for an edition: [`Limits::java`] for Java, [`Limits::default`] for Bedrock.
    #[must_use]
    pub fn for_edition(edition: Edition) -> Self {
        match edition {
            Edition::Java => Self::java(),
            Edition::Bedrock => Self::default(),
        }
    }
}

/// Detects the edition from the content (magic bytes) and the file name.
///
/// gzip or zlib compression means Java; an uncompressed root compound looks the same in both editions (`0a 00 00`), so only the
/// file name can tell them apart.
/// Returns `None` when it cannot tell, leaving it to the caller to ask. It does not guess: handing an arbitrary file to one of the
/// parsers only produces confusing errors.
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

/// Detects the edition and parses. Returns [`Error::UnknownFormat`] when the edition cannot be detected.
pub fn parse(bytes: &[u8], filename: Option<&str>, limits: &Limits) -> Result<Structure, Error> {
    match detect(bytes, filename) {
        Some(Edition::Bedrock) => parse_bedrock(bytes, limits),
        Some(Edition::Java) => parse_java(bytes, limits),
        None => Err(Error::UnknownFormat),
    }
}

/// Parses a Bedrock `.mcstructure`.
pub fn parse_bedrock(bytes: &[u8], limits: &Limits) -> Result<Structure, Error> {
    bedrock::parse(bytes, limits)
}

/// Parses a Java structure file: a vanilla structure `.nbt` or a Litematica schematic `.litematic` (gzip, zlib or uncompressed).
/// The format is decided by **content**, not by extension: a root with `Regions` is a schematic, a root with any of `size`,
/// `blocks`, `palette` or `palettes` is a vanilla structure,
/// and anything else (for example `.schem`) returns [`Error::UnknownFormat`]. When the extension and the content disagree (a schematic
/// renamed to `.nbt`), the content wins and [`Structure::format`] reports what was actually parsed.
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
