# mc-structure

A parser for Minecraft structure files: **Bedrock `.mcstructure`**, **Java vanilla structures `.nbt`** and **Litematica
schematics `.litematic`**, all parsed into one structure model. It was written for the submission review of
[Redstone Archive](https://redstonearchive.top) and is published as a standalone crate.

```rust
let bytes = std::fs::read("door_close.mcstructure")?;
let s = mc_structure::parse(&bytes, Some("door_close.mcstructure"), &mc_structure::Limits::default())?;
println!("{:?}, {} blocks", s.size, s.block_count());
for (name, n) in s.materials().iter().take(5) {
    println!("{name} × {n}");
}
let same = s.digest() == other.digest(); // do two files hold the same structure?
let d = close.diff(&open);               // how many cells differ between the open and closed states?
```

```text
$ cargo run --example inspect -- tests/fixtures/bedrock_v1_close.mcstructure
edition    bedrock (mcstructure, format_version 1)
size       4 × 17 × 14
origin     [-12, 104, 180]
blocks     685 (palette 88, block entities 188, entities 0)
```

## Supported formats

| Edition | Format | Notes |
| --- | --- | --- |
| Bedrock | `.mcstructure`, `format_version` 1 | `block_indices` is a `List<List<Int>>` with two layers (layer 1 is the waterlogging layer) |
| Bedrock | `.mcstructure`, `format_version` 2 | `block_indices` is a `List<IntArray>`; only one layer has been observed |
| Java | Vanilla structure `.nbt` | gzip, zlib or uncompressed; `palette` or `palettes` (the first one is used) |
| Java | Litematica schematic `.litematic` | gzip-compressed big-endian NBT with one or more regions (`Position`, a `Size` that may be negative), each with its own palette and bit-packed `BlockStates` (tightly packed across long boundaries, unlike chunk storage since 1.16). Format versions 4 to 7 are read the same way |

Block names stay as each edition spells them. **This crate does not map blocks between Bedrock and Java**; that is left to
whatever renders them.

Java files are told apart **by content, not by extension**: a root with `Regions` is a schematic; a root with any of `size`,
`blocks`, `palette` or `palettes` is a vanilla structure; anything else (such as `.schem`) returns `Error::UnknownFormat`.
When the extension and the content disagree (a schematic renamed to `.nbt`), the content wins, and `Structure::format`
reports what was parsed (`nbt` / `litematic` / `mcstructure`, matching each format's extension).

### How a schematic becomes a `Structure`

- Region bounds: on each axis `min = Position + (Size < 0 ? Size + 1 : 0)` and the length is `|Size|`. The union of all
  region bounds is the bounds of the structure, with the minimum corner moved to 0,0,0.
- Each region is placed at its own offset. When two regions both have a non-air block in the same cell, the later region
  wins; air never overwrites anything. The palettes of all regions are merged into one.
- Block entity positions (relative to the region's minimum corner) and entity `Pos` values get the region offset; the
  region-relative `x`, `y`, `z` are removed from block entity data, since the position lives in `pos`.
- `TotalBlocks` and `EnclosingSize` in `Metadata` are informational and ignored in favour of the decoded data;
  `PendingBlockTicks` and similar lists are skipped.
- One structure saved as `.nbt` or as `.litematic` has the same `digest()`, `block_count()` and `materials()`, so
  duplicate detection and "are both states the same build" checks work across formats.

## Model

- `Structure`: edition, format, `format_version` (`DataVersion` for Java), size, origin (Bedrock), palette, non-air blocks
  sorted by position, block entities and entities.
- `BlockState::key()`: `name` or `name[k=v,…]` with states sorted by key, so each state has exactly one spelling.
- `digest()`: SHA-256 over the size and the position and spelling of every non-empty cell. Independent of palette order
  and compression.
- `materials()`: block id → count, by count descending.
- `diff()`: number of cells that differ between two states (a block on one side and none on the other, or a different
  block or state). Compares blocks only, not entities.

Bedrock's linear index unfolds with z varying fastest, then y, then x: `i = x·(sy·sz) + y·sz + z`. In the fixtures, all 295
block entities record a world position equal to "structure origin + position in the structure"; the test
`bedrock_block_entity_world_coords_match_origin_plus_pos` uses this to check the unfolding order.

## Untrusted input

One NBT reader serves both editions, with the byte order selecting the variant (Java: big-endian, modified UTF-8;
Bedrock: little-endian, UTF-8). All safety limits live in one place:

| Limit | Default |
| --- | --- |
| Nesting depth | 64 |
| Total elements (arrays and lists count their length; also the volume limit) | 8,000,000 |
| Size after decompression | 64 MiB |
| Size along one axis | 10,000 |
| Memory (estimated, checked before allocating) | 112 MiB, counting 32 bytes per tag plus the heap part of strings, arrays, lists and compounds. Counting bytes rather than elements matters: a list element of 1 byte in the file is a 32-byte tag in memory, so together with gzip a few KB could grow to hundreds of MB. The default fits the largest Bedrock files in the old format (`List<Int>` block indices; two layers of 64×384×64 take about 96 MiB). `Limits::java()` lowers the budget for Java files, which never need that much |

Schematics are checked further: the `|Size|` of each region and the combined bounds stay within the axis limit on every
axis; the summed volume of the regions and the volume of the combined bounds stay within the element limit; and there
are at most 64 regions. All of this is checked while reading the headers, before any `BlockStates` is decoded. A
`BlockStates` array must hold at least `ceil(volume × bits / 64)` longs; the length is checked before decoding, so nothing is
allocated from sizes the file merely claims.

Every declared length is checked against the remaining input and the element budget before allocating. Errors carry the
byte offset and the NBT path (`structure.block_indices[0] at offset 1234: unexpected end of data`). Nothing on the parse
path panics.

## Tests

- Golden tests (`tests/fixtures.rs`): expected values are computed from the same files by an **independent Python reference
  implementation** (`tools/nbt_le.py`, `tools/golden.py`, sharing no code with the Rust crate). When the normalization rules
  change, change both and recompute with `python3 tools/golden.py tests/fixtures/*.mcstructure`.
- Schematic tests (`tests/litematic.rs`): fixtures and expected values come from `tools/litematic.py`. They cover the same
  door saved as `.nbt` and as `.litematic` (versions 6 and 7, positive and negative sizes, different palette orders), a
  multi-region schematic (overlapping regions, a negative size, block entities, entities, deliberately wrong `Metadata`)
  and its flattened `.nbt`, and palettes of 17 and 64 entries (5 and 6 bits, spanning long boundaries). The script decodes
  bit-packed states by treating the whole long array as one big integer, while the crate shifts and joins neighbouring
  longs, so agreement between them is meaningful. Schematics are also built on the fly: a packer that places one bit at a
  time covers palettes from 1 to 1000 entries (with positive and negative sizes), plus malformed files (wrong lengths,
  indices out of range, oversized regions, too many regions, missing parts, block entities outside their region).
  Recompute expected values with `python3 tools/litematic.py read tests/fixtures/*.litematic tests/fixtures/*.nbt`;
  regenerate the fixtures with `python3 tools/litematic.py gen tests/fixtures`.
- NBT round trips, truncation, depth bombs, length bombs, decompression bombs, and random bytes that must not panic.
- `tests/memory.rs`: memory accounting against compressed inputs that expand to millions of tags.

## Fixtures

`tests/fixtures/*.mcstructure` are the closed states of two redstone doors submitted by players to Redstone Archive.
When replacing them, recompute the expected values with `tools/golden.py`.

`door_*`, `multi_region*` and `palette*` (`.nbt` and `.litematic`) in `tests/fixtures/` are small structures generated by
`tools/litematic.py`.

`tests/fixtures/sunspire_palace.litematic` is a file **exported by Litematica itself** (the author in its metadata is
yeaminist): format version 6, a single region of 70×46×66, a palette of 715 entries (10 bits per index, spanning long
boundaries) and 1825 block entities. It pins the implementation down in three ways: the decoded count of non-air blocks,
21459, matches the `TotalBlocks` that Litematica wrote into the metadata, and the size matches `EnclosingSize`; and every
one of the 1825 block entities (whose positions the file records separately) lands on a block that has a block entity,
such as a shelf, sign, barrel or chest, never on air. With a wrong cell order (x and y swapped, say) 1629 of them would
land on air.

## Compatibility with other readers

Compared with other `.litematic` readers, this crate matches the usual decoding rules: bits per index, indices spanning two
longs, cell order, the minimum corner of negative sizes, skipping air, and later regions overwriting earlier ones. It
accepts two common variations: block entities stored under `BlockEntities` instead of `TileEntities`, and `BlockStates`
arrays with extra trailing longs (too few still fails). It is stricter in two places, because silently dropping blocks
would make a broken file look valid: a region with an empty palette or no data is an error rather than skipped, and so is a
cell whose palette index is out of range. The size is the region bounds (consistent with Litematica's `EnclosingSize` and
with a vanilla structure's `size`), not the bounding box of the non-air blocks.

## Toolchain and license

Minimum Rust 1.91.1; 1.94 recommended. Licensed under the [MIT license](LICENSE).
