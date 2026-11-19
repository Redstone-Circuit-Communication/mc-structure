//! NBT reading and writing. One reader serves both editions; the byte order selects Java (big-endian, modified UTF-8) or
//! Bedrock (little-endian, UTF-8).
//!
//! All safety limits live here: nesting depth; an element budget (**arrays and lists count their length**, otherwise an
//! IntArray declaring tens of millions of elements would take a few bytes and never reach the limit); a **memory budget**
//! (the bytes the parsed tags take in memory, checked before allocating);
//! and every length is checked against the remaining input before allocating. Nothing on the parse path panics.
//! Errors carry the byte offset and the tag path; the path is assembled only while unwinding an error, so the success path does
//! not allocate for it.

use std::fmt;

use crate::Limits;

/// Byte order (which also decides the string encoding).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    /// Java Edition: big-endian, strings in modified UTF-8
    Big,
    /// Bedrock Edition: little-endian, strings in UTF-8
    Little,
}

/// An NBT value.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    /// TAG_Byte
    Byte(i8),
    /// TAG_Short
    Short(i16),
    /// TAG_Int
    Int(i32),
    /// TAG_Long
    Long(i64),
    /// TAG_Float
    Float(f32),
    /// TAG_Double
    Double(f64),
    /// TAG_Byte_Array
    ByteArray(Vec<i8>),
    /// TAG_String
    String(String),
    /// TAG_List (all elements of the same type)
    List(Vec<Tag>),
    /// TAG_Compound
    Compound(Compound),
    /// TAG_Int_Array
    IntArray(Vec<i32>),
    /// TAG_Long_Array
    LongArray(Vec<i64>),
}

/// A compound tag, keeping the order of the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compound(pub Vec<(String, Tag)>);

impl Compound {
    /// Looks up a key (compounds are usually small, so a linear scan is enough).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Iterates over the key-value pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Tag)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Number of keys.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the compound is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Tag {
    /// Tag type id.
    #[must_use]
    pub const fn id(&self) -> u8 {
        match self {
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }

    /// Reads Byte, Short and Int as an integer.
    #[must_use]
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Byte(v) => Some(i32::from(*v)),
            Self::Short(v) => Some(i32::from(*v)),
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// The string value.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// The compound value.
    #[must_use]
    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Self::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// The list value.
    #[must_use]
    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Self::List(v) => Some(v),
            _ => None,
        }
    }

    /// A sequence of integers: accepts `List<Int/Short/Byte>` as well as `IntArray` (Bedrock's `block_indices` uses the former
    /// in `format_version` 1 and the latter in 2).
    #[must_use]
    pub fn as_ints(&self) -> Option<Vec<i32>> {
        match self {
            Self::IntArray(v) => Some(v.clone()),
            Self::List(v) => v.iter().map(Self::as_i32).collect(),
            _ => None,
        }
    }

    /// Type name, for error messages.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Byte(_) => "Byte",
            Self::Short(_) => "Short",
            Self::Int(_) => "Int",
            Self::Long(_) => "Long",
            Self::Float(_) => "Float",
            Self::Double(_) => "Double",
            Self::ByteArray(_) => "ByteArray",
            Self::String(_) => "String",
            Self::List(_) => "List",
            Self::Compound(_) => "Compound",
            Self::IntArray(_) => "IntArray",
            Self::LongArray(_) => "LongArray",
        }
    }
}

/// What went wrong.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NbtErrorKind {
    /// The data ended early
    #[error("unexpected end of data")]
    Truncated,
    /// An unknown tag type id
    #[error("unknown tag type {0}")]
    UnknownTag(u8),
    /// The root tag is not a compound (most likely not an NBT file, or the wrong byte order)
    #[error("root tag is not a compound")]
    RootNotCompound,
    /// Nested too deeply
    #[error("nesting deeper than {0} levels")]
    TooDeep(u32),
    /// Too many elements
    #[error("more than {0} elements")]
    TooManyElements(u64),
    /// The parsed tags would take more memory than allowed (counted in bytes: a list element of 1 byte in the file is a
    /// 32-byte tag in memory)
    #[error("parsed data would take more than {0} bytes of memory")]
    TooMuchMemory(u64),
    /// A declared length is negative
    #[error("negative length")]
    NegativeLength,
    /// A declared length is larger than the remaining input
    #[error("declared length {0} exceeds the remaining data")]
    LengthExceedsInput(u64),
    /// A string is not validly encoded
    #[error("invalid string encoding")]
    BadString,
    /// A non-empty list has element type End
    #[error("non-empty list with element type End")]
    ListOfEnd,
}

/// An NBT error: what went wrong, the byte offset and the tag path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NbtError {
    /// Read position at the time of the error
    pub offset: usize,
    /// What went wrong
    pub kind: NbtErrorKind,
    /// Path segments, innermost first (`path()` joins them into `a.b[3].c`)
    segments: Vec<Seg>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Seg {
    Key(String),
    Index(usize),
}

impl NbtError {
    fn new(offset: usize, kind: NbtErrorKind) -> Self {
        Self { offset, kind, segments: Vec::new() }
    }

    fn at(mut self, seg: Seg) -> Self {
        self.segments.push(seg);
        self
    }

    /// The tag path, such as `structure.block_indices[0]`; empty for an error at the root.
    #[must_use]
    pub fn path(&self) -> String {
        let mut out = String::new();
        for s in self.segments.iter().rev() {
            match s {
                Seg::Key(k) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(k);
                }
                Seg::Index(i) => out.push_str(&format!("[{i}]")),
            }
        }
        out
    }
}

impl fmt::Display for NbtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = self.path();
        let p = if p.is_empty() { "<root>" } else { p.as_str() };
        write!(f, "{p} at offset {}: {}", self.offset, self.kind)
    }
}

impl std::error::Error for NbtError {}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    endian: Endian,
    limits: Limits,
    elements: u64,
    /// Memory accounted so far (bytes, estimated)
    memory: u64,
}

type R<T> = Result<T, NbtError>;

/// Bytes one tag takes in memory (every list element is one)
const TAG_BYTES: u64 = std::mem::size_of::<Tag>() as u64;
/// Bytes one compound entry (key plus value) takes in memory; the key's characters are counted separately
const ENTRY_BYTES: u64 = std::mem::size_of::<(String, Tag)>() as u64;

impl<'a> Reader<'a> {
    fn err(&self, kind: NbtErrorKind) -> NbtError {
        NbtError::new(self.pos, kind)
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> R<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or_else(|| self.err(NbtErrorKind::Truncated))?;
        let s = self.buf.get(self.pos..end).ok_or_else(|| self.err(NbtErrorKind::Truncated))?;
        self.pos = end;
        Ok(s)
    }

    fn arr<const N: usize>(&mut self) -> R<[u8; N]> {
        let s = self.take(N)?;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }

    fn u8(&mut self) -> R<u8> {
        Ok(self.arr::<1>()?[0])
    }
}

macro_rules! num {
    ($name:ident, $t:ty, $n:literal) => {
        fn $name(&mut self) -> R<$t> {
            let a = self.arr::<$n>()?;
            Ok(match self.endian {
                Endian::Big => <$t>::from_be_bytes(a),
                Endian::Little => <$t>::from_le_bytes(a),
            })
        }
    };
}

impl Reader<'_> {
    num!(i8, i8, 1);
    num!(i16, i16, 2);
    num!(u16, u16, 2);
    num!(i32, i32, 4);
    num!(i64, i64, 8);
    num!(f32, f32, 4);
    num!(f64, f64, 8);

    /// Element budget: each tag counts 1, arrays and lists count their length.
    fn bump(&mut self, n: u64) -> R<()> {
        self.elements = self.elements.saturating_add(n);
        if self.elements > self.limits.max_elements {
            return Err(self.err(NbtErrorKind::TooManyElements(self.limits.max_elements)));
        }
        Ok(())
    }

    /// Memory budget: roughly how many bytes the parsed tags take in memory, recorded **before** allocating. Counted in bytes,
    /// not in elements: a list element of 1 byte in the file is a 32-byte `Tag` in memory, so counting elements alone would let a
    /// few KB of gzip grow to hundreds of MB.
    fn charge(&mut self, bytes: u64) -> R<()> {
        self.memory = self.memory.saturating_add(bytes);
        if self.memory > self.limits.max_memory {
            return Err(self.err(NbtErrorKind::TooMuchMemory(self.limits.max_memory)));
        }
        Ok(())
    }

    /// Reads a length and checks it: non-negative, and no more elements than the remaining bytes can hold. When `budget` is
    /// true the length is charged to the element budget
    /// (for arrays; not for lists, whose elements each count 1 as they are read, so charging here would count them twice).
    fn len(&mut self, elem_size: usize, budget: bool) -> R<usize> {
        let n = self.i32()?;
        let n = usize::try_from(n).map_err(|_| self.err(NbtErrorKind::NegativeLength))?;
        if budget {
            self.bump(n as u64)?;
        }
        if elem_size > 0 && n > self.remaining() / elem_size {
            return Err(self.err(NbtErrorKind::LengthExceedsInput(n as u64)));
        }
        Ok(n)
    }

    fn string(&mut self) -> R<String> {
        let n = usize::from(self.u16()?);
        self.charge(n as u64)?;
        let start = self.pos;
        let bytes = self.take(n)?;
        let s = match self.endian {
            Endian::Little => std::str::from_utf8(bytes).map(str::to_owned).ok(),
            Endian::Big => cesu8::from_java_cesu8(bytes).map(std::borrow::Cow::into_owned).ok(),
        };
        s.ok_or_else(|| NbtError::new(start, NbtErrorKind::BadString))
    }

    fn payload(&mut self, id: u8, depth: u32) -> R<Tag> {
        self.bump(1)?;
        Ok(match id {
            1 => Tag::Byte(self.i8()?),
            2 => Tag::Short(self.i16()?),
            3 => Tag::Int(self.i32()?),
            4 => Tag::Long(self.i64()?),
            5 => Tag::Float(self.f32()?),
            6 => Tag::Double(self.f64()?),
            7 => {
                let n = self.len(1, true)?;
                self.charge(n as u64)?;
                Tag::ByteArray(self.take(n)?.iter().map(|b| i8::from_ne_bytes([*b])).collect())
            }
            8 => Tag::String(self.string()?),
            9 => self.list(depth)?,
            10 => Tag::Compound(self.compound(depth)?),
            11 => {
                let n = self.len(4, true)?;
                self.charge(n as u64 * 4)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.i32()?);
                }
                Tag::IntArray(v)
            }
            12 => {
                let n = self.len(8, true)?;
                self.charge(n as u64 * 8)?;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.i64()?);
                }
                Tag::LongArray(v)
            }
            other => return Err(self.err(NbtErrorKind::UnknownTag(other))),
        })
    }

    fn deeper(&self, depth: u32) -> R<u32> {
        let d = depth + 1;
        if d > self.limits.max_depth {
            return Err(self.err(NbtErrorKind::TooDeep(self.limits.max_depth)));
        }
        Ok(d)
    }

    fn list(&mut self, depth: u32) -> R<Tag> {
        let depth = self.deeper(depth)?;
        let elem = self.u8()?;
        // Minimum bytes per element, used to check the declared length before allocating
        let min = match elem {
            0 => 0,
            1 => 1,
            2 => 2,
            3 | 5 => 4,
            4 | 6 => 8,
            8 => 2,
            7 | 9 | 11 | 12 => 4,
            10 => 1,
            other => return Err(self.err(NbtErrorKind::UnknownTag(other))),
        };
        let n = self.len(min, false)?;
        if elem == 0 && n > 0 {
            return Err(self.err(NbtErrorKind::ListOfEnd));
        }
        // Account for the memory before allocating: every element is a full Tag
        self.charge(n as u64 * TAG_BYTES)?;
        let mut v = Vec::with_capacity(n);
        for i in 0..n {
            v.push(self.payload(elem, depth).map_err(|e| e.at(Seg::Index(i)))?);
        }
        Ok(Tag::List(v))
    }

    fn compound(&mut self, depth: u32) -> R<Compound> {
        let depth = self.deeper(depth)?;
        let mut out = Vec::new();
        loop {
            let id = self.u8()?;
            if id == 0 {
                return Ok(Compound(out));
            }
            let key = self.string()?;
            self.charge(ENTRY_BYTES)?;
            match self.payload(id, depth) {
                Ok(v) => out.push((key, v)),
                Err(e) => return Err(e.at(Seg::Key(key))),
            }
        }
    }
}

/// Reads an NBT document whose root is a compound, returning the root name and the root compound. Trailing bytes after the
/// root are ignored.
pub fn read(buf: &[u8], endian: Endian, limits: &Limits) -> Result<(String, Compound), NbtError> {
    let mut r = Reader { buf, pos: 0, endian, limits: *limits, elements: 0, memory: 0 };
    if r.u8()? != 10 {
        return Err(NbtError::new(0, NbtErrorKind::RootNotCompound));
    }
    let name = r.string()?;
    let root = r.compound(0)?;
    Ok((name, root))
}

/// Writes an NBT document whose root is a compound (used for test fixtures and tools). A list is written with the type of its
/// first element.
#[must_use]
pub fn write(name: &str, root: &Compound, endian: Endian) -> Vec<u8> {
    let mut w = Writer { out: Vec::new(), endian };
    w.out.push(10);
    w.string(name);
    w.compound(root);
    w.out
}

struct Writer {
    out: Vec<u8>,
    endian: Endian,
}

macro_rules! put {
    ($self:ident, $v:expr) => {
        match $self.endian {
            Endian::Big => $self.out.extend_from_slice(&($v).to_be_bytes()),
            Endian::Little => $self.out.extend_from_slice(&($v).to_le_bytes()),
        }
    };
}

impl Writer {
    fn len(&mut self, n: usize) {
        put!(self, i32::try_from(n).unwrap_or(i32::MAX));
    }

    fn string(&mut self, s: &str) {
        let bytes: Vec<u8> = match self.endian {
            Endian::Big => cesu8::to_java_cesu8(s).into_owned(),
            Endian::Little => s.as_bytes().to_vec(),
        };
        put!(self, u16::try_from(bytes.len()).unwrap_or(u16::MAX));
        self.out.extend_from_slice(&bytes);
    }

    fn compound(&mut self, c: &Compound) {
        for (k, v) in &c.0 {
            self.out.push(v.id());
            self.string(k);
            self.payload(v);
        }
        self.out.push(0);
    }

    fn payload(&mut self, t: &Tag) {
        match t {
            Tag::Byte(v) => put!(self, *v),
            Tag::Short(v) => put!(self, *v),
            Tag::Int(v) => put!(self, *v),
            Tag::Long(v) => put!(self, *v),
            Tag::Float(v) => put!(self, *v),
            Tag::Double(v) => put!(self, *v),
            Tag::ByteArray(v) => {
                self.len(v.len());
                self.out.extend(v.iter().map(|b| b.to_ne_bytes()[0]));
            }
            Tag::String(s) => self.string(s),
            Tag::List(v) => {
                self.out.push(v.first().map_or(0, Tag::id));
                self.len(v.len());
                for x in v {
                    self.payload(x);
                }
            }
            Tag::Compound(c) => self.compound(c),
            Tag::IntArray(v) => {
                self.len(v.len());
                for x in v {
                    put!(self, *x);
                }
            }
            Tag::LongArray(v) => {
                self.len(v.len());
                for x in v {
                    put!(self, *x);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Compound {
        Compound(vec![
            ("b".into(), Tag::Byte(-3)),
            ("s".into(), Tag::Short(-300)),
            ("i".into(), Tag::Int(123_456)),
            ("l".into(), Tag::Long(-9_000_000_000)),
            ("f".into(), Tag::Float(1.5)),
            ("d".into(), Tag::Double(-2.25)),
            ("ba".into(), Tag::ByteArray(vec![1, -1, 0])),
            ("str".into(), Tag::String("红石 \u{0}🙂".into())),
            ("list".into(), Tag::List(vec![Tag::Int(1), Tag::Int(2)])),
            ("empty".into(), Tag::List(vec![])),
            ("nested".into(), Tag::Compound(Compound(vec![("k".into(), Tag::String("v".into()))]))),
            ("ia".into(), Tag::IntArray(vec![-1, 0, 7])),
            ("la".into(), Tag::LongArray(vec![i64::MIN, i64::MAX])),
        ])
    }

    #[test]
    fn roundtrip_both_endians() {
        for e in [Endian::Big, Endian::Little] {
            let bytes = write("root", &sample(), e);
            let (name, back) = read(&bytes, e, &Limits::default()).unwrap();
            assert_eq!(name, "root");
            assert_eq!(back, sample());
        }
    }

    #[test]
    fn java_strings_are_modified_utf8() {
        // Modified UTF-8: U+0000 becomes C0 80, and characters outside the BMP become surrogate pairs (6 bytes)
        let bytes = write("", &Compound(vec![("s".into(), Tag::String("\u{0}🙂".into()))]), Endian::Big);
        assert!(bytes.windows(2).any(|w| w == [0xc0, 0x80]));
        assert!(!bytes.contains(&0xf0), "4-byte UTF-8 sequences must not appear");
    }

    #[test]
    fn truncation_reports_offset_and_path() {
        let bytes = write("", &sample(), Endian::Little);
        let cut = &bytes[..bytes.len() - 10];
        let e = read(cut, Endian::Little, &Limits::default()).unwrap_err();
        // Cut in the middle of the last LongArray: the declared length exceeds the remaining bytes and is rejected before
        // allocating (LengthExceedsInput); a cut inside a fixed-size field is Truncated instead. Both report incomplete input;
        // what matters is that they carry the offset and the path.
        assert!(matches!(e.kind, NbtErrorKind::Truncated | NbtErrorKind::LengthExceedsInput(_)), "{:?}", e.kind);
        assert!(e.path().starts_with("la"), "the path should point at the last field: {}", e.path());
    }

    #[test]
    fn array_length_counts_against_budget() {
        // An IntArray declaring 1000 elements: with a budget of 100 it is rejected before allocating
        let mut bytes = vec![10, 0, 0, 11, 1, 0, b'a'];
        bytes.extend_from_slice(&1000i32.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, 4000));
        bytes.push(0);
        let limits = Limits { max_elements: 100, ..Limits::default() };
        let e = read(&bytes, Endian::Little, &limits).unwrap_err();
        assert_eq!(e.kind, NbtErrorKind::TooManyElements(100));
    }

    /// NBT holding a single byte list: each element takes 1 byte in the file
    fn byte_list(n: i32) -> Vec<u8> {
        let mut bytes = vec![10, 0, 0, 9, 0, 1, b'l', 1];
        bytes.extend_from_slice(&n.to_be_bytes());
        bytes.resize(bytes.len() + usize::try_from(n).unwrap(), 0);
        bytes.push(0);
        bytes
    }

    #[test]
    fn memory_is_charged_in_bytes_before_allocation() {
        // A list element of 1 byte in the file is a 32-byte tag in memory. Four million of them need about 122 MiB, more than
        // the default 112 MiB, so they are rejected before allocating (counting elements alone would have let such a file take
        // about 128 MB)
        let e = read(&byte_list(4_000_000), Endian::Big, &Limits::default()).unwrap_err();
        assert_eq!(e.kind, NbtErrorKind::TooMuchMemory(Limits::default().max_memory));
        // A small one reads normally; with a smaller budget the same data is rejected too
        let (_, root) = read(&byte_list(1000), Endian::Big, &Limits::default()).unwrap();
        assert_eq!(root.get("l").and_then(Tag::as_list).map(<[Tag]>::len), Some(1000));
        let tight = Limits { max_memory: 10_000, ..Limits::default() };
        assert!(matches!(read(&byte_list(1000), Endian::Big, &tight).unwrap_err().kind, NbtErrorKind::TooMuchMemory(10_000)));
    }

    #[test]
    fn list_elements_count_once_against_the_element_budget() {
        // The list length must not be charged on top of the elements themselves, or the effective limit would be halved
        let limits = Limits { max_elements: 1000 + 1, ..Limits::default() };
        assert!(read(&byte_list(1000), Endian::Big, &limits).is_ok(), "1000 elements plus the list itself is exactly 1001");
        let limits = Limits { max_elements: 1000, ..Limits::default() };
        assert!(matches!(read(&byte_list(1000), Endian::Big, &limits).unwrap_err().kind, NbtErrorKind::TooManyElements(1000)));
    }

    #[test]
    fn declared_length_beyond_input_is_rejected_before_allocation() {
        let mut bytes = vec![10, 0, 0, 12, 1, 0, b'x'];
        bytes.extend_from_slice(&(i32::MAX).to_le_bytes());
        let limits = Limits { max_elements: u64::MAX, ..Limits::default() };
        let e = read(&bytes, Endian::Little, &limits).unwrap_err();
        assert!(matches!(e.kind, NbtErrorKind::LengthExceedsInput(_)));
    }

    #[test]
    fn depth_bomb_is_rejected() {
        // Lists nested 100 levels deep
        let mut bytes = vec![10, 0, 0, 9, 1, 0, b'x'];
        for _ in 0..100 {
            bytes.push(9);
            bytes.extend_from_slice(&1i32.to_le_bytes());
        }
        let e = read(&bytes, Endian::Little, &Limits::default()).unwrap_err();
        assert_eq!(e.kind, NbtErrorKind::TooDeep(64));
    }

    #[test]
    fn negative_length_and_unknown_tag() {
        let mut bytes = vec![10, 0, 0, 11, 1, 0, b'a'];
        bytes.extend_from_slice(&(-1i32).to_le_bytes());
        assert_eq!(read(&bytes, Endian::Little, &Limits::default()).unwrap_err().kind, NbtErrorKind::NegativeLength);
        let bytes = [10, 0, 0, 13, 1, 0, b'a'];
        assert_eq!(read(&bytes, Endian::Little, &Limits::default()).unwrap_err().kind, NbtErrorKind::UnknownTag(13));
        assert_eq!(read(&[8, 0, 0], Endian::Little, &Limits::default()).unwrap_err().kind, NbtErrorKind::RootNotCompound);
    }

    /// Random bytes must neither panic nor hang (deterministic pseudo-random input, a lightweight stand-in for fuzzing; see the
    /// README for the cargo-fuzz target).
    #[test]
    fn random_bytes_never_panic() {
        let mut s: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let base = write("", &sample(), Endian::Little);
        for round in 0..2000 {
            let mut b = base.clone();
            let flips = 1 + (next() % 8) as usize;
            for _ in 0..flips {
                let i = (next() as usize) % b.len();
                b[i] = next() as u8;
            }
            if round % 3 == 0 {
                b.truncate((next() as usize) % b.len());
            }
            let _ = read(&b, Endian::Little, &Limits::default());
            let _ = read(&b, Endian::Big, &Limits::default());
        }
    }
}
