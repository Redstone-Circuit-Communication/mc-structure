# mc-structure

[English](README.md) | 简体中文

Minecraft 结构文件的解析库：**基岩版 `.mcstructure`**、**Java 版原版结构 `.nbt`** 和 **Litematica 投影文件 `.litematic`**，
都解析成同一个结构模型。它是为[红石阁](https://redstonearchive.top)的投稿审核写的，作为独立的 crate 发布。

```rust
let bytes = std::fs::read("door_close.mcstructure")?;
let s = mc_structure::parse(&bytes, Some("door_close.mcstructure"), &mc_structure::Limits::default())?;
println!("{:?}, {} blocks", s.size, s.block_count());
for (name, n) in s.materials().iter().take(5) {
    println!("{name} × {n}");
}
let same = s.digest() == other.digest(); // 两个文件存的是不是同一个结构？
let d = close.diff(&open);               // 开、关两种状态之间有多少格不同？
```

```text
$ cargo run --example inspect -- tests/fixtures/bedrock_v1_close.mcstructure
edition    bedrock (mcstructure, format_version 1)
size       4 × 17 × 14
origin     [-12, 104, 180]
blocks     685 (palette 88, block entities 188, entities 0)
```

## 支持的格式

| 版本 | 格式 | 说明 |
| --- | --- | --- |
| 基岩版 | `.mcstructure`，`format_version` 1 | `block_indices` 是两层的 `List<List<Int>>`（第 1 层是含水层） |
| 基岩版 | `.mcstructure`，`format_version` 2 | `block_indices` 是 `List<IntArray>`；目前只见过一层 |
| Java 版 | 原版结构 `.nbt` | gzip、zlib 或未压缩；`palette` 或 `palettes`（取第一个） |
| Java 版 | Litematica 投影 `.litematic` | gzip 压缩的大端 NBT，包含一个或多个区域（`Position`，`Size` 可以为负），每个区域有自己的调色板和按位打包的 `BlockStates`（跨 long 边界紧密排列，和 1.16 起区块存储的方式不同）。格式版本 4 到 7 按同样的方式读取 |

方块名保留各自版本的写法。**本库不在基岩版和 Java 版之间做方块映射**，这件事留给负责渲染的一方。

Java 文件**按内容区分，不看扩展名**：根标签里有 `Regions` 的是投影文件；有 `size`、`blocks`、`palette`、`palettes` 任意一个的
是原版结构；其他的（比如 `.schem`）返回 `Error::UnknownFormat`。扩展名和内容对不上时（投影文件改名成了 `.nbt`）以内容为准，
`Structure::format` 报告实际解析出的格式（`nbt` / `litematic` / `mcstructure`，和各格式的扩展名一致）。

### 投影文件怎样变成 `Structure`

- 区域边界：每个轴上 `min = Position + (Size < 0 ? Size + 1 : 0)`，长度是 `|Size|`。所有区域边界的并集就是结构的边界，
  最小角移到 0,0,0。
- 每个区域放在自己的偏移位置上。两个区域在同一格都有非空气方块时，后面的区域优先；空气不会覆盖任何东西。
  所有区域的调色板合并成一个。
- 方块实体的位置（相对于区域的最小角）和实体的 `Pos` 加上区域偏移；方块实体数据里相对于区域的 `x`、`y`、`z` 会被删掉，
  因为位置放在 `pos` 里。
- `Metadata` 里的 `TotalBlocks` 和 `EnclosingSize` 只作参考，以解码出的数据为准；`PendingBlockTicks` 之类的列表会跳过。
- 同一个结构存成 `.nbt` 或 `.litematic`，`digest()`、`block_count()` 和 `materials()` 都相同，
  所以查重和「两种状态是不是同一个作品」的检查可以跨格式进行。

## 模型

- `Structure`：版本、格式、`format_version`（Java 版是 `DataVersion`）、尺寸、原点（基岩版）、调色板、
  按位置排序的非空气方块、方块实体和实体。
- `BlockState::key()`：`name` 或按键排序状态的 `name[k=v,…]`，所以每种状态只有一种写法。
- `digest()`：对尺寸以及每个非空格子的位置和写法做 SHA-256。与调色板顺序和压缩方式无关。
- `materials()`：方块 id → 数量，按数量从多到少。
- `diff()`：两种状态之间不同的格子数（一边有方块另一边没有，或者方块、状态不同）。只比较方块，不比较实体。

基岩版的线性下标按 z 变化最快、其次 y、最后 x 展开：`i = x·(sy·sz) + y·sz + z`。测试文件里全部 295 个方块实体记录的世界坐标，
都等于「结构原点 + 结构内的位置」；测试 `bedrock_block_entity_world_coords_match_origin_plus_pos` 用这一点检查展开顺序。

## 不可信的输入

两个版本共用一个 NBT 读取器，按字节序区分（Java 版：大端、修改版 UTF-8；基岩版：小端、UTF-8）。所有安全限制集中在一处：

| 限制 | 默认值 |
| --- | --- |
| 嵌套深度 | 64 |
| 元素总数（数组和列表按长度计；同时也是体积上限） | 8,000,000 |
| 解压后的大小 | 64 MiB |
| 单个轴上的尺寸 | 10,000 |
| 内存（估算，在分配之前检查） | 112 MiB，每个标签按 32 字节计，再加上字符串、数组、列表和复合标签在堆上的部分。按字节而不是按元素计数很重要：文件里 1 字节的列表元素在内存里是一个 32 字节的标签，再加上 gzip，几 KB 的文件可能膨胀到几百 MB。默认值能容下旧格式最大的基岩版文件（`List<Int>` 形式的方块下标；两层 64×384×64 约占 96 MiB）。`Limits::java()` 给 Java 文件用更低的预算，它们从来用不到这么多 |

投影文件还有更多检查：每个区域的 `|Size|` 和合并后的边界在每个轴上都不超过单轴上限；所有区域的体积之和、合并边界的体积
都不超过元素上限；区域最多 64 个。这些都在读取头部时检查，在解码任何 `BlockStates` 之前。`BlockStates` 数组至少要有
`ceil(体积 × 位数 / 64)` 个 long；长度在解码之前检查，所以不会按文件里自称的大小去分配内存。

每个声明的长度都会先和剩余输入、元素预算比较，再分配内存。错误信息带有字节偏移和 NBT 路径
（`structure.block_indices[0] at offset 1234: unexpected end of data`）。解析路径上没有任何地方会 panic。

## 测试

- 黄金测试（`tests/fixtures.rs`）：期望值由一个**独立的 Python 参考实现**（`tools/nbt_le.py`、`tools/golden.py`，
  和 Rust 代码不共用任何代码）从同样的文件算出。规范化规则变了时，两边一起改，再用
  `python3 tools/golden.py tests/fixtures/*.mcstructure` 重新计算。
- 投影文件测试（`tests/litematic.rs`）：测试文件和期望值来自 `tools/litematic.py`。覆盖同一扇门分别存成 `.nbt` 和 `.litematic`
  （版本 6 和 7、正负尺寸、不同的调色板顺序），一个多区域投影（区域重叠、负尺寸、方块实体、实体、故意写错的 `Metadata`）
  及其展平后的 `.nbt`，以及 17 项和 64 项的调色板（5 位和 6 位，跨 long 边界）。脚本把整个 long 数组当成一个大整数来解码
  按位打包的状态，而 crate 是对相邻的 long 移位拼接，所以两者一致是有意义的。投影文件也会在测试中现场生成：一个逐位放置的
  打包器覆盖 1 到 1000 项的调色板（正负尺寸都有），以及各种格式错误的文件（长度不对、下标越界、区域过大、区域过多、
  缺少部分内容、方块实体在区域之外）。用 `python3 tools/litematic.py read tests/fixtures/*.litematic tests/fixtures/*.nbt`
  重新计算期望值；用 `python3 tools/litematic.py gen tests/fixtures` 重新生成测试文件。
- NBT 往返、截断、深度炸弹、长度炸弹、解压炸弹，以及不能导致 panic 的随机字节。
- `tests/memory.rs`：针对会膨胀成数百万个标签的压缩输入，检查内存估算。

## 测试文件

`tests/fixtures/*.mcstructure` 是玩家投稿到红石阁的两扇红石门的关闭状态。替换它们时，用 `tools/golden.py` 重新计算期望值。

`tests/fixtures/` 里的 `door_*`、`multi_region*`、`palette*` 和 `large_region`（`.nbt` 和 `.litematic`）由 `tools/litematic.py` 生成。

同样是生成的 `tests/fixtures/large_region.litematic` 有一个小型作品那么大：一个 40×20×36 的区域，700 项的调色板
（每个下标 10 位，跨 long 边界），以及放在箱子、木桶和告示牌上的 390 个方块实体，元数据按 Litematica 的写法写入。
它从三个方面把实现钉住：解码出的非空气方块数 15872 和元数据里的 `TotalBlocks` 一致，尺寸和 `EnclosingSize` 一致；
每个方块实体（文件里单独记录了位置）都落在箱子、木桶或告示牌上。如果格子顺序错了（比如 x 和 y 对调），
大部分方块实体会落在空气或音符盒上。

## 和其他读取器的兼容性

和其他 `.litematic` 读取器相比，本库遵循通常的解码规则：每个下标的位数、跨两个 long 的下标、格子顺序、负尺寸的最小角、
跳过空气、后面的区域覆盖前面的。它接受两种常见的变体：方块实体存在 `BlockEntities` 而不是 `TileEntities` 下，
以及 `BlockStates` 数组末尾多出 long（太少仍然报错）。有两处更严格，因为悄悄丢掉方块会让坏文件看起来是好的：
调色板为空或没有数据的区域是错误而不是跳过，调色板下标越界的格子也是错误。尺寸是区域的边界（和 Litematica 的
`EnclosingSize`、原版结构的 `size` 一致），不是非空气方块的包围盒。

## 工具链和许可

Rust 最低 1.91.1，建议 1.94。以 [MIT 许可](LICENSE)发布。
