# mc-structure

Minecraft 结构文件解析库：**基岩版 `.mcstructure`**、**Java 版原版结构 `.nbt`** 与 **Litematica 投影 `.litematic`** → 同一个结构模型。
它是[在造世界 · 红石阁](https://redstonearchive.top)后端用来审核投稿的那一层，单独开源。

```rust
let bytes = std::fs::read("door_close.mcstructure")?;
let s = mc_structure::parse(&bytes, Some("door_close.mcstructure"), &mc_structure::Limits::default())?;
println!("{:?} 共 {} 个方块", s.size, s.block_count());
for (name, n) in s.materials().iter().take(5) {
    println!("{name} × {n}");
}
let same = s.digest() == other.digest();      // 两个文件是不是同一个结构
let d = close.diff(&open);                     // 开、关两态有多少格不同
```

```text
$ cargo run --example inspect -- tests/fixtures/bedrock_v1_close.mcstructure
版本      bedrock (mcstructure, format_version 1)
尺寸      4 × 17 × 14
方块      685（调色板 88 项，方块实体 188，实体 0）
```

## 支持的格式

| 版本 | 格式 | 说明 |
| --- | --- | --- |
| 基岩版 | `.mcstructure` `format_version` 1 | `block_indices` 是 `List<List<Int>>`，两层（第 1 层是含水层） |
| 基岩版 | `.mcstructure` `format_version` 2 | `block_indices` 是 `List<IntArray>`，实测只有一层 |
| Java 版 | 原版结构 `.nbt` | gzip、zlib 或未压缩；`palette` 或 `palettes`（取第一个） |
| Java 版 | Litematica 投影 `.litematic` | gzip 的大端 NBT；一个或多个区域（`Position`、可以是负数的 `Size`），每个区域一张调色板加位压缩的 `BlockStates`（跨 long 的边界紧密打包，不是 1.16 以后区块里的写法）。第 4 到 7 版都按同一种读法 |

方块名保持各自版本的原样，**本库不做基岩与 Java 之间的方块映射**（两个版本的渲染各自处理）。

Java 的文件**按内容分流，不按扩展名**：根上有 `Regions` 的是投影；有 `size`、`blocks`、`palette`、`palettes` 之一的是原版结构；都不是（比如 `.schem`）返回 `Error::UnknownFormat`。
扩展名和内容对不上（把投影改名成 `.nbt`）照内容解析，`Structure::format` 以内容为准（`nbt` / `litematic` / `mcstructure`，正好是各自的扩展名）。

### 投影怎么变成 `Structure`

- 区域的范围：每个轴 `min = Position + (Size < 0 ? Size + 1 : 0)`，长度 `|Size|`；全部区域的范围合起来就是整个结构（最小角移到 0,0,0）。
- 多个区域各按自己的偏移放进去，两个区域在同一格都有非空方块时后面的覆盖前面的，空气不覆盖任何东西；各区域的调色板合并成一张。
- 方块实体的坐标（相对区域的最小角）和实体的 `Pos` 都加上区域的偏移；方块实体数据里区域内的 `x`、`y`、`z` 不再留（位置在 `pos` 里）。
- `Metadata` 里的 `TotalBlocks`、`EnclosingSize` 只是参考，不当真，以解码出来的为准；`PendingBlockTicks` 这些不管。
- 同一个结构存成 `.nbt` 或 `.litematic`，`digest()`、`block_count()`、`materials()` 都一样（查重、「两态是不是同一个结构」可以跨格式比较）。

## 模型

- `Structure`：版本、格式、`format_version`（Java 是 DataVersion）、尺寸、原点（基岩）、调色板、非空方块（按坐标排序）、方块实体、实体。
- `BlockState::key()`：`name` 或 `name[k=v,…]`，状态按键排序，同一状态只有一种写法。
- `digest()`：尺寸与每个非空格子的「坐标 + 写法」的 SHA-256。与调色板顺序、压缩方式无关。
- `materials()`：方块 id → 数量，按数量降序。
- `diff()`：两态之间不同的格子数（一边有方块另一边没有，或方块/状态不同），只比方块不比实体。

基岩的线性下标按 z 最快、然后 y、然后 x 展开：`i = x·(sy·sz) + y·sz + z`。夹具里全部 295 个方块实体记着的世界坐标
都等于「结构原点 + 结构内坐标」，测试 `bedrock_block_entity_world_coords_match_origin_plus_pos` 用它验证展开顺序。

## 不可信输入

一个 NBT 读取器，用字节序参数区分两个版本（Java：大端、修改版 UTF-8；基岩：小端、UTF-8），安全上限只有一份：

| 上限 | 默认 |
| --- | --- |
| 嵌套深度 | 64 |
| 元素总数（数组与列表按长度计入；也是体积上限） | 8,000,000 |
| 解压后大小 | 64 MiB |
| 单轴尺寸 | 10,000 |
| 内存（估算，分配之前核对） | 112 MiB：每个标签算 32 字节，加上字符串、数组、列表、复合标签在堆上的部分。按字节记、不按个数：线上 1 字节的列表元素在内存里是一个 32 字节的标签，再加上 gzip，几 KB 的文件能变成上百 MB。默认值装得下本站上限的旧版基岩文件（`List<Int>` 的方块下标，64×384×64 两层约 96 MiB） |

投影另有：每个区域的 `|Size|` 与合起来的范围，每个轴都不超过单轴尺寸；各区域的体积加起来、合起来的范围的体积，都不超过元素总数；区域数不超过 64。
这些都在读头部时检查，通过了才去解码 `BlockStates`；`BlockStates` 的长度必须正好是 `ceil(体积 × 位数 / 64)`，先核对再解码，不按文件里声称的尺寸直接分配。

声明的长度先对照剩余字节和元素预算再分配；错误带字节偏移与 NBT 路径（`structure.block_indices[0] @ 偏移 1234：数据提前结束`）；解析路径上不 panic。

## 测试

- 黄金测试（`tests/fixtures.rs`）：期望值由**独立的 Python 参考实现**（`tools/nbt_le.py`、`tools/golden.py`，与 Rust 代码不共享任何东西）
  从同样的文件算出。改了规范化规则，两边一起改，`python3 tools/golden.py tests/fixtures/*.mcstructure` 重新出数。
- 投影的测试（`tests/litematic.rs`）：夹具由 `tools/litematic.py` 生成并算出期望值——同一扇门分别存成 `.nbt` 和 `.litematic`（第 6、7 版，正负尺寸，调色板顺序不同），
  多区域（重叠、负尺寸、方块实体、实体、故意写错的 Metadata）和它压平后的 `.nbt`，调色板 17 项、64 项的（位数 5、6，跨 long 的边界）。
  那个脚本读投影时把整个 long 数组当成一个大整数按位切，库里是两个 long 之间移位拼接，两边读出来一致才有意义。
  另外现场造投影：一个一位一位放的打包器覆盖 1 到 1000 项的调色板（正尺寸与负尺寸各一遍），以及各种坏文件（长度不对、下标越界、区域超大、区域太多、缺东西、方块实体越界）。
  `python3 tools/litematic.py read tests/fixtures/*.litematic tests/fixtures/*.nbt` 重新出数；`python3 tools/litematic.py gen tests/fixtures` 重新生成夹具。
- NBT 往返、截断、深度炸弹、长度炸弹、解压炸弹、随机字节不 panic。
- 计划：`cargo-fuzz` 目标（需要 nightly，单独的 CI 任务）。

## 夹具

`tests/fixtures/*.mcstructure` 是在造世界 · 红石阁玩家投稿的两个红石门的关闭态，按本站的投稿协议使用。
换夹具时用 `tools/golden.py` 重新算期望值。

`tests/fixtures/` 里的 `door_*`、`multi_region*`、`palette*`（`.nbt` 与 `.litematic`）是 `tools/litematic.py` 造的小结构，没有玩家的东西。

`tests/fixtures/sunspire_palace.litematic` 是 **Litematica 真导出的文件**（站长提供，元数据里的作者是 yeaminist）：第 6 版、单区域 70×46×66、
调色板 715 项（每个下标 10 位，跨 long 的边界）、1825 个方块实体。它把实现钉住了三处：
解出的非空方块数 21459 与 Litematica 自己写在元数据里的 `TotalBlocks` 一致，尺寸与 `EnclosingSize` 一致；
1825 个方块实体（坐标是文件里另记的）所在的格子解出来全是架子、告示牌、木桶、箱子这类带方块实体的方块，没有一个是空气——
格子顺序弄错（比如 x、y 互换）会有 1629 个落在空气上。**开源之前要确认这个文件可以随库分发**（作者的许可），不行就换成自己存的投影。

和站长以前的项目（一个网页上的 schematic 查看器，`parseLitematicFile`）的解析逻辑对照过：每个下标的位数、跨 long 的两段拼接、格子顺序、
负尺寸的最小角、跳过空气、后面的区域覆盖前面的，都一样。它比本库宽松的两处本库跟进了：方块实体的键也认 `BlockEntities`；`BlockStates` 多出来的 long 不管（少了照样报错）。
另外两处本库保留更严的做法：空调色板、空数据的区域，它跳过、本库报错；调色板下标越界的格子，它跳过、本库报错（投稿要做校验，悄悄丢方块会让结果看起来没问题）。
尺寸它按方块裁掉四周的空气，本库按区域的范围算（与 Litematica 的 `EnclosingSize`、原版结构的 `size` 一致）。

## 工具链与许可证

最低 Rust 1.91.1，推荐 1.94。MIT。
