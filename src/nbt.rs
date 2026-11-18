//! NBT 读写：一个读取器，用字节序参数区分 Java（大端、修改版 UTF-8）与基岩（小端、UTF-8）。
//!
//! 安全上限只在这里有一份：嵌套深度、元素预算（**数组与列表按长度计入**——否则一个声明了几千万元素的
//! IntArray 只占几个字节，再大的结构也碰不到上限）、**内存预算**（按解析出来的标签在内存里的字节数记，分配之前核对）、
//! 分配前对照剩余字节。解析路径上不 panic。
//! 错误带字节偏移与路径，路径只在出错回溯时拼出来，正常路径不多分配。

use std::fmt;

use crate::Limits;

/// 字节序（也决定字符串编码）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    /// Java 版：大端，字符串为修改版 UTF-8
    Big,
    /// 基岩版：小端，字符串为 UTF-8
    Little,
}

/// 一个 NBT 值。
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
    /// TAG_List（元素类型相同）
    List(Vec<Tag>),
    /// TAG_Compound
    Compound(Compound),
    /// TAG_Int_Array
    IntArray(Vec<i32>),
    /// TAG_Long_Array
    LongArray(Vec<i64>),
}

/// 复合标签：保持文件里的顺序。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Compound(pub Vec<(String, Tag)>);

impl Compound {
    /// 按键取值（复合标签一般很小，线性查找即可）。
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Tag> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// 遍历键值对。
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Tag)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// 键的个数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Tag {
    /// 标签类型号。
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

    /// Byte、Short、Int 都当整数读。
    #[must_use]
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Self::Byte(v) => Some(i32::from(*v)),
            Self::Short(v) => Some(i32::from(*v)),
            Self::Int(v) => Some(*v),
            _ => None,
        }
    }

    /// 字符串。
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// 复合标签。
    #[must_use]
    pub fn as_compound(&self) -> Option<&Compound> {
        match self {
            Self::Compound(c) => Some(c),
            _ => None,
        }
    }

    /// 列表。
    #[must_use]
    pub fn as_list(&self) -> Option<&[Tag]> {
        match self {
            Self::List(v) => Some(v),
            _ => None,
        }
    }

    /// 整数序列：`List<Int/Short/Byte>` 或 `IntArray` 都接受（基岩 `format_version` 1 与 2 的 `block_indices` 就分别是这两种）。
    #[must_use]
    pub fn as_ints(&self) -> Option<Vec<i32>> {
        match self {
            Self::IntArray(v) => Some(v.clone()),
            Self::List(v) => v.iter().map(Self::as_i32).collect(),
            _ => None,
        }
    }

    /// 类型名（错误信息用）。
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

/// 出错的原因。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NbtErrorKind {
    /// 数据提前结束
    #[error("数据提前结束")]
    Truncated,
    /// 不认识的标签类型号
    #[error("未知的标签类型 {0}")]
    UnknownTag(u8),
    /// 根标签不是复合标签（多半不是 NBT 文件，或字节序选错了）
    #[error("根标签不是复合标签")]
    RootNotCompound,
    /// 嵌套太深
    #[error("嵌套超过 {0} 层")]
    TooDeep(u32),
    /// 元素太多
    #[error("元素超过 {0} 个")]
    TooManyElements(u64),
    /// 解析出来的标签要占的内存超过上限（按字节记：线上 1 字节的列表元素在内存里是 32 字节的标签）
    #[error("解析出来要占的内存超过 {0} 字节")]
    TooMuchMemory(u64),
    /// 声明的长度为负数
    #[error("长度为负数")]
    NegativeLength,
    /// 声明的长度比剩下的字节还多
    #[error("声明的长度 {0} 超过剩余数据")]
    LengthExceedsInput(u64),
    /// 字符串编码不对
    #[error("字符串编码不对")]
    BadString,
    /// 非空列表的元素类型是 End
    #[error("非空列表的元素类型是 End")]
    ListOfEnd,
}

/// NBT 错误：原因 + 字节偏移 + 路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NbtError {
    /// 出错时读到的位置
    pub offset: usize,
    /// 原因
    pub kind: NbtErrorKind,
    /// 路径段，最内层在前（`path()` 拼成 `a.b[3].c`）
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

    /// 路径，如 `structure.block_indices[0]`；根上出错是空串。
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
        let p = if p.is_empty() { "<根>" } else { p.as_str() };
        write!(f, "{p} @ 偏移 {}：{}", self.offset, self.kind)
    }
}

impl std::error::Error for NbtError {}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    endian: Endian,
    limits: Limits,
    elements: u64,
    /// 已经记下的内存（字节，估算）
    memory: u64,
}

type R<T> = Result<T, NbtError>;

/// 一个标签本身在内存里占的字节（列表的每个元素都是一个）
const TAG_BYTES: u64 = std::mem::size_of::<Tag>() as u64;
/// 复合标签里的一项（键加值）本身占的字节；键的字符串内容另算
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

    /// 元素预算：每个标签算 1，数组与列表按长度算。
    fn bump(&mut self, n: u64) -> R<()> {
        self.elements = self.elements.saturating_add(n);
        if self.elements > self.limits.max_elements {
            return Err(self.err(NbtErrorKind::TooManyElements(self.limits.max_elements)));
        }
        Ok(())
    }

    /// 内存预算：解析出来的标签在内存里大约占多少字节，在**分配之前**记上。按字节记，不按个数
    /// （线上 1 字节的列表元素在内存里是一个 32 字节的 `Tag`；只按个数记，gzip 过的几 KB 文件能变成上百 MB）。
    fn charge(&mut self, bytes: u64) -> R<()> {
        self.memory = self.memory.saturating_add(bytes);
        if self.memory > self.limits.max_memory {
            return Err(self.err(NbtErrorKind::TooMuchMemory(self.limits.max_memory)));
        }
        Ok(())
    }

    /// 读一个长度并核对：非负、不超过剩余字节能装下的个数。`budget` 为真时把长度计入元素预算
    /// （数组要；列表不要——列表的每个元素读的时候各自算 1，以前在这里再算一遍，等于记了两次）。
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
        // 每个元素至少占的字节数：用来在分配前核对声明的长度
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
        // 先记内存再分配：每个元素都是一个完整的 Tag
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

/// 读一个以复合标签为根的 NBT 文档，返回根名与根复合标签。根之后多余的字节忽略。
pub fn read(buf: &[u8], endian: Endian, limits: &Limits) -> Result<(String, Compound), NbtError> {
    let mut r = Reader { buf, pos: 0, endian, limits: *limits, elements: 0, memory: 0 };
    if r.u8()? != 10 {
        return Err(NbtError::new(0, NbtErrorKind::RootNotCompound));
    }
    let name = r.string()?;
    let root = r.compound(0)?;
    Ok((name, root))
}

/// 写一个以复合标签为根的 NBT 文档（测试夹具与工具用）。列表按第一个元素的类型写。
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
        // 修改版 UTF-8：U+0000 编成 C0 80，增补平面字符编成代理对（6 字节）
        let bytes = write("", &Compound(vec![("s".into(), Tag::String("\u{0}🙂".into()))]), Endian::Big);
        assert!(bytes.windows(2).any(|w| w == [0xc0, 0x80]));
        assert!(!bytes.contains(&0xf0), "不应出现 4 字节 UTF-8");
    }

    #[test]
    fn truncation_reports_offset_and_path() {
        let bytes = write("", &sample(), Endian::Little);
        let cut = &bytes[..bytes.len() - 10];
        let e = read(cut, Endian::Little, &Limits::default()).unwrap_err();
        // 截在最后一个 LongArray 中间：声明的长度超过剩下的字节，分配之前就拒绝（LengthExceedsInput）；
        // 截在定长字段中间则是 Truncated。两种都如实说明「输入不完整」，关键是带上偏移与路径。
        assert!(matches!(e.kind, NbtErrorKind::Truncated | NbtErrorKind::LengthExceedsInput(_)), "{:?}", e.kind);
        assert!(e.path().starts_with("la"), "路径应指向最后一个字段：{}", e.path());
    }

    #[test]
    fn array_length_counts_against_budget() {
        // 一个声明了 1000 个元素的 IntArray：预算 100 时在分配之前就拒绝
        let mut bytes = vec![10, 0, 0, 11, 1, 0, b'a'];
        bytes.extend_from_slice(&1000i32.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, 4000));
        bytes.push(0);
        let limits = Limits { max_elements: 100, ..Limits::default() };
        let e = read(&bytes, Endian::Little, &limits).unwrap_err();
        assert_eq!(e.kind, NbtErrorKind::TooManyElements(100));
    }

    /// 只含一个字节列表的 NBT：线上每个元素 1 字节
    fn byte_list(n: i32) -> Vec<u8> {
        let mut bytes = vec![10, 0, 0, 9, 0, 1, b'l', 1];
        bytes.extend_from_slice(&n.to_be_bytes());
        bytes.resize(bytes.len() + usize::try_from(n).unwrap(), 0);
        bytes.push(0);
        bytes
    }

    #[test]
    fn memory_is_charged_in_bytes_before_allocation() {
        // 评审 M1：线上 1 字节的列表元素在内存里是一个 32 字节的标签。400 万个要约 122 MiB，超过默认的 112 MiB，
        // 分配之前就拒绝（以前按个数记，这样的文件能占到约 128 MB）
        let e = read(&byte_list(4_000_000), Endian::Big, &Limits::default()).unwrap_err();
        assert_eq!(e.kind, NbtErrorKind::TooMuchMemory(Limits::default().max_memory));
        // 小的照常读；预算调小了同样的东西也会被拒
        let (_, root) = read(&byte_list(1000), Endian::Big, &Limits::default()).unwrap();
        assert_eq!(root.get("l").and_then(Tag::as_list).map(<[Tag]>::len), Some(1000));
        let tight = Limits { max_memory: 10_000, ..Limits::default() };
        assert!(matches!(read(&byte_list(1000), Endian::Big, &tight).unwrap_err().kind, NbtErrorKind::TooMuchMemory(10_000)));
    }

    #[test]
    fn list_elements_count_once_against_the_element_budget() {
        // 以前列表的长度先算一遍、每个元素读的时候又算一遍：实际上限只有一半
        let limits = Limits { max_elements: 1000 + 1, ..Limits::default() };
        assert!(read(&byte_list(1000), Endian::Big, &limits).is_ok(), "1000 个元素加上列表本身，正好 1001");
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
        // 100 层嵌套的列表
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

    /// 随机字节：只要不 panic、不挂住（确定性的伪随机，模糊测试的轻量替身；完整的 cargo-fuzz 目标见 README）
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
