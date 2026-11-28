Status: proposed (as of 2026-11-27) - audit and design only; no code. Governed by the rule in section R. Main path: hardware texture formats (section 3). Decisions are in section 11.

# Compression in SceneDB, and neural texture compression: plan

## R. The rule (non-negotiable)

From the user, 2026-11-27: "currently everything is very nice to work with
because it's generic and you just read from AGPU buffer. We can't break that.
That is forbidden." And on packing: "We can still pack them but not in scenedb
whoever owns the pass and component pair that write and read the data needs to
do it".

**For buffer data:**

1. **SceneDB never packs, encodes or generates read functions.** It stores the
   element type it is given, and reading it stays a plain generic buffer read.
   No SceneDB macro option, enum kind or registry applies to buffer packing.
2. **The owner of the component that writes the data and the pass that reads
   it may pack it.** The packed layout is then their own row contract: they
   write it and they unpack it.
3. **With other readers, the contract lives where Helio's rules put it**: the
   consumer's api crate owns the row ABI, with layout tests on both sides and
   one WGSL unpack snippet that others include. No copies (4.2).

**For textures** (as before): the shader's declarations, bindings and sample
calls stay as they are, and **every channel a sample returns is the channel it
returns today**, up to the error of a lossy format. No channel is dropped,
moved, replaced by a constant that differs from the source, or left for the
shader to reconstruct. A texture design that needs otherwise is ruled out.
Whether an owner may choose otherwise for its own textures, as for its own
buffers, is not settled (decision 1).

Existing encodings (normals packed in `PackedVertex`, the virtual-geometry
cluster format) are their owners' contracts already.

**How a SceneDB change proves it keeps the rule:** consumers' WGSL is
unchanged by it; buffer keys, element types and row layouts are identical
(existing layout tests, unchanged); and for a texture format, a test shows
each sampled channel matches the source within the quality threshold at every
mip. An owner's packing is instead a deliberate change to that owner's
contract, proven by 4.2's tests.

## Summary

- **Textures can be compressed within the rule.** Shaders already receive a
  texture as a `u32` slot in a row and sample it through a table (1.4). A
  hardware block format changes only what is stored behind the slot.
- **Only formats that keep all four channels qualify** (3.1). A 4-bit format
  qualifies only for textures whose alpha is 1 everywhere. Single and
  two-channel formats are out.
- **SceneDB can apply it when a field opts in**, with a runtime API under the
  macro, but the opt-in must fill the `u32` the reader already reads (3.3).
- **SceneDB does not pack buffer data; a data's owner may** (section 4).
  SceneDB's part is lossless compression undone before upload, and spending
  less memory on capacity and duplicates. For the mesh vertex pool, packing is
  the meshes plugin's own contract change, and touches raster layouts, two
  acceleration-structure builders, two copies of the shader decode, CPU
  readers and the mesh file format (4.3).
- **Neural texture compression cannot save video memory under this rule.**
  Its memory-saving modes put a decoder or a page lookup in the shader. What
  is left, decoding at load into ordinary block textures, only shrinks stored
  and shipped bytes (section 6). I recommend not building it now.
- **At most about 1.0 GiB** of the 2.04 GiB of textures in the screenshot
  could be saved, and only if all of them qualified for a 4-bit format, which
  they will not (1.0, 3.5).

The file keeps its original name; its subject is now compression in SceneDB,
with neural compression assessed in section 6.

## 0. Findings (read these first)

1. **Hardware block formats are texture formats.** They do not apply to
   storage or vertex buffers.
2. **Every `#[gpu]` field in SceneDB today is buffer data.** Textures exist
   through `TextureStore`, a runtime API no field maps to.
3. **A macro alone would miss material textures.** They are named inside
   material assets, and their use is worked out from the graph at run time. If
   the screenshot's 565 textures are material textures (likely for the Bistro,
   not established), a field opt-in by itself would not change them.
4. **Hardware decodes; it does not encode.** An encoder, a cache and a policy
   for changing data are part of the feature.
5. **The derive silently ignores a field attribute it cannot parse** (1.2). A
   new option mistyped, or built against an older SceneDB, would compile and
   do nothing.
6. **SceneDB itself compresses nothing, but much of what it holds is not raw**
   (1.1). Material textures are already BC7 on devices that sample BC, for
   sizes that are multiples of 4.
7. **A packaged game ships source images inside a compressed pak.**
   `pulsar_package` copies assets as authored (texture compression is named as
   future work). The pak compresses each block with zstd, or stores it raw
   when that is not smaller, then encrypts it. There is no cooked texture in a
   package yet.
8. **Texture compression reaches about a third of the screenshot's video
   memory.** Unused vertex pool space and free driver blocks together are over
   half the size of the textures, and are not compression problems.

## 1. Audit

### 1.0 The video memory picture that prompted this

A VRAM Map screenshot from the user (editor, 2026-11-27). Sizes are read off
the image; derived figures are my arithmetic on its rounded values.

| Part | Size | Reached by texture compression? |
|---|---|---|
| Allocated / reserved in 26 driver blocks | 5.34 / 6.13 GiB | |
| SceneDB, "Scene Texture" x565 | 2.04 GiB (38% of allocated, 33% of reserved) | **yes** |
| SceneDB, `builtin_mesh_vertex` pools x6 | 940.4 MiB at 53% used: about 498 MiB live, 442 MiB not | no |
| SceneDB, the rest (index pools and smaller) | about 227 MiB (3.18 GiB less the two above) | no |
| Helio | 1.58 GiB (targets, histories, atlases, caches) | not by this plan |
| Unscoped | 463.3 MiB | no |
| Renderer host, meshes (nearly all BLAS) | 143.4 MiB | no |
| Free in driver blocks | 799.0 MiB | no |

The header also says owners report 418.2 MiB live of 768.2 MiB claimed (54%).
The textures report no use, so the picture does not show which are sampled.

**Format of those textures: very likely BC7, not proven by the image.** The
hovered 5.4 MiB fits both a 2048x2048 BC7 chain (5,592,432 bytes) and a
1024x1024 RGBA8 chain (5,592,404 bytes), and the allocation carries only the
label "Scene Texture". The rest of the evidence:

- in production code that label is set only in
  `helio_component/src/texture_assets.rs`, with the cooked texture's format;
- engine devices request BC compression where the adapter has it
  (`render_runtimes/device.rs`), and `CookFormat::best_for` then picks BC7 for
  any size that is a multiple of 4;
- `MEMORY.md` measured 565 textures for the Bistro: 2.07 GB as BC7, 8.26 GB
  as RGBA8. The screenshot's 565 in 2.04 GiB matches the first;
- in the cooked-texture headers of one local derived cache
  (`tb_perf_project_r2`), 2048x2048 BC7 files hold 5.33 MiB and RGBA8 ones
  21.33 MiB. No 1024x1024 group was among its 30 most numerous format, use and
  size groups. The cache holds both formats, and I did not establish that it
  belongs to the project in the screenshot.

So the large saving from ordinary block compression appears to be taken. A
total of 2.04 GiB equals about 390 chains of 5.33 MiB, so sizes are mixed.

**The pool slack.** The two largest pools, 320.1 MiB each, and the next,
160.1 MiB, match 2^23 and 2^22 vertices of the 40-byte `PackedVertex`, as
capacity doubling would give (`DynamicGpuBuffer::ensure_capacity`).
`shrink_to_fit` exists on that buffer, but I found no call to it from
`VarLenGpuPool`; freed ranges inside a pool can be reused but do not return
memory. How much of the 47% is headroom and how much holes, and why there are
six pools (`MEMORY.md` says pools are per scene by design), were not
determined. Free driver blocks are the GPU allocator's. Two unscoped labels
(`filter_group_texture`, `persistent_framebuffer`) are created in the UI
framework (`crates/ui/wgpui`); `VRAM.md`'s 0.5 MB unscoped figure is for the
Bistro in the game and does not describe this editor picture.

### 1.1 Is everything raw today?

| Data | Today | Who does it |
|---|---|---|
| Component rows, var-len pools and their uploads | written as handed in; I found no encoding step | SceneDB |
| RAM staging and the "Disk" spill arena | raw bytes | SceneDB |
| `derive_cached` files | a header (8 reserved zero bytes) plus the payload as handed in | SceneDB |
| Dependencies | no compression crate | `pulsar_scenedb`'s `Cargo.toml`; a source search found none |
| Network replication | `ReplicationEncoding`, a per-field wire enum, with `DeltaCompressed` (XOR and LEB128) | SceneDB. Wire only |
| Material textures in VRAM | **BC7**, or RGBA8 as fallback | cooked by `helio_component`, held in `TextureStore` |
| `PackedVertex` (40 bytes) | f32 position and UVs; normal and tangent packed into a u32 each | meshes |
| Virtual-geometry clusters | quantized positions and attributes | meshes (`vg/format`) |
| Packaged content | zstd blocks (`Codec { Raw, Zstd }`), then encrypted | `pulsar_content` |

### 1.2 What `#[gpu]` is today

| Declaration | Becomes | Source (`pulsar_scenedb_derive`) |
|---|---|---|
| `#[gpu]` fixed-size field | its own storage-buffer column, or part of one packed record (`layout = packed`) | `scene_store.rs`, `gpu.rs` |
| `#[gpu] Vec<T>` or `Arc<[T]>` | a shared variable-length pool plus a handle column; `content_id` interns it | `var_len.rs` |
| `GpuHeavy<H>` placements | pool elements are lightweight references | `scene_store.rs` |
| `#[gpu(mirror = Once, heavy)]` | one fixed-size element made by `GpuUploadSource` at write time | `gpu.rs` |

Field options are `mirror`, `buffer`, `heavy`, `content_id`.

- **A field attribute that fails to parse is silently ignored.** The parser has
  an "unknown `#[gpu]` option" error, but the field loop reads it with
  `if let Ok(gpu_attr) = attr.parse_args()`, so the field is then mirrored as
  a plain `#[gpu]` column. The struct-level attribute is lenient too. A bare
  `#[gpu]` also fails that parse, and can be matched as its own case.
- **The macro works on names, not types** (`Vec`, `GpuHeavy`, `AssetId`). Type
  rules have to be trait bounds in generated code, as `heavy` does.
- **The mirror dispatch runs inside the world write**, on the writing thread.
  It cannot load or encode.
- **There are two derives.** `engine_class_derive` classifies `#[gpu]` fields
  and refuses `Vec` and `GpuHeavy` unless the struct routes to `SceneStore`.

### 1.3 Textures from file to GPU

| Step | What happens | Where |
|---|---|---|
| Reference | A component or material graph names an image file. Sources stay in the project; there is no texture asset or import. | components; `materials/component/src/graph.rs` |
| Use | For materials, `texture_uses` gives each image Color, Data or Normal ("strongest" wins). | `graph.rs` |
| Cook | Decode, cap at `streaming.max_texture_size` (2048), full mips (colour in linear light; **normal mips keep the mean normal's length in alpha**), then BC7 for every use, or RGBA8 without BC or when a side is not a multiple of 4. CPU, Intel ISPC (`intel_tex_2`). | `cooked_texture.rs`, `helio_core::texture_mips` |
| Cache | `derive_cached` on disk, `Residency<CookedTexture>` in RAM; coarse tier (128 px) first | `texture_assets.rs` |
| Upload | `register_mips_staged` into `TextureStore` under a per-frame budget; one GPU texture per (device, asset, use) | `texture_assets.rs`, SceneDB `gpu/assets.rs` |
| VRAM budget | One process-wide mip bias (`texture_detail.rs`, 3072 MB) | `helio_component` |
| Package | Source assets into a zstd, encrypted pak | `pulsar_package`, `pulsar_content` |

Images, cooking and residency are core by an explicit decision (Helio
`plugin-migration.md`).

### 1.4 How a shader gets a texture today (traced)

1. A component names an image. SceneDB's asset path loads it on a worker and
   binds it into the component.
2. Row code asks `texture_assets::texture_slot` (materials:
   `resident_slot_staged`) for the image's `TextureStore` slot and writes that
   **`u32` into a row**.
3. The shader reads the `u32` from the buffer and samples `scene_textures[slot]`
   with the shared sampler.

| Reader | Where the slot sits | Channels it uses |
|---|---|---|
| Material evaluator (all classic drawers; compute stages through `explicit_gradients`) | `tex_*` in the material row, or `MaterialTextureData.texture_index` | base colour rgba; normal rgb **and a** (specular anti-aliasing); ORM g and b; occlusion r; emissive rgb; specular colour rgb; **specular weight a** |
| `vg_alpha.wgsl` | the same rows | base colour a |
| Voxel `scene_materials.wgsl` | `tex_*` | colour, ORM, normal rgb and a; also `textureDimensions` |
| Decals | `albedo_texture_index`, `normal_texture_index` in the pass row. The source row is **packed**, written by a hand-written dispatch, then joined | albedo rgba; normal rgb |
| Sprites | `texture: [u32; 4]` in a **packed** source row, hand-written dispatch | rgba |
| Cables | a word in the definition buffer | rgba (alpha cuts out) |
| UI | `p.info.y` | rgba |

So **the user's understanding holds for table textures**: if the image behind a
slot changes format and keeps its channels, the buffer read and the sample are
both unchanged. "Nothing" is not one value everywhere (components write
`u32::MAX`; the table reserves slot 0 as white; row clears write zeros), and
each row's current convention has to be kept.

Some textures are not table textures (the sky panorama opens and binds its own
image). Their readers were not traced; the field kind below does not cover
them. I found no shader reading table textures' bytes other than by sampling.

### 1.5 What SceneDB offers to build on

| Part | Today | Limits |
|---|---|---|
| Asset references in fields | the derive collects `AssetId` fields for the handle ledger | |
| Loading and re-binding | asset pump on workers; `register_binding` | |
| Textures | `TextureStore`: any 2D format, staged budgeted upload, in-place slot replacement, `register_shared`, reserved slots | whole textures only; no content dedup or leases; `MipSource` gives finished CPU bytes |
| CPU and disk caches | `Residency<T>`, `derive_cached` | `derive_cached` keys one source file and prunes |
| Tiers | `TierEngine` for pool ranges and rows | "Disk" is an in-process arena; no mip concept; texture materialization is uncompressed, whole, one per pool |

`helio_component` assembles per-asset slots, per-device sharing (two
process-wide static tables), tiered loading, cooking and the detail bias.

Pulsar configures the tiers with no materializations, and a search of
`crates/core`, `crates/editor`, `plugins/vendor` and Helio's crates found no
caller of `touch_tier` or `release_tier`; `TextureStore` takes no part in the
tier budget. `MEMORY.md` calls `streaming.texture_stream_pool_mb` that
engine's VRAM budget; from this reading it does not bound texture VRAM
(follow-up, 11).

## 2. "OS-allocator-style" transparent compression

The original idea: SceneDB compresses transparently, as an OS compresses
memory. OS compression is lossless, page-granular, and a touch of a compressed
page **faults** into code that restores it first.

- A shader's load or texture fetch cannot fault into engine code. Even tiled
  resources only report a miss for the shader to handle; this wgpu exposes
  neither tiled resources nor sampler feedback.
- A shader computes addresses, so resident data must be fixed-rate. Lossless
  streams are not.
- What the GPU does decode invisibly is fixed-function: **texture block
  formats**, through the sampler. (Drivers also compress some surfaces
  losslessly for bandwidth; that is not ours to control.)

So within the rule there are exactly two things: **bytes that are restored
before the GPU reader can see them**, and **texture formats the sampler
decodes**. The second is not byte-exact, and something has to encode it.

## 3. Main path: hardware texture formats, opted in on a field

The user's direction: lean on hardware decoding, and have SceneDB apply it
automatically when a field opts in through the macro. **Verdict: feasible for
table textures, within the rule, with the limits below.**

| The opt-in can make automatic | It cannot |
|---|---|
| format choice per device; encode through an installed encoder; disk cache; deduplication; budgeted upload; slot lifetime; size reporting; fallback | encode GPU-written data; change a shader; compress buffer data; give a 4-bit format to a texture that uses alpha |

### 3.1 Which formats keep the rule

Bits per texel in parentheses, from block sizes (general knowledge).

| What the texture is (decided by inspecting every mip) | BC | ASTC | ETC2 family | Uncompressed fallback |
|---|---|---|---|---|
| LDR with alpha that is not 1 everywhere | BC7 (8) | 4x4 (8), up to 8x8 (2) | RGBA8 (8) | RGBA8 (32) |
| LDR with alpha 1 everywhere | BC1 (4) or BC7 (8), by tier | block sizes (8 down to 2), alpha 1 | RGB8 (4) or RGBA8 (8) | RGBA8 (32) |
| HDR, non-negative, alpha 1 | BC6H (8) | HDR profile, needs its feature | none | RGB9E5 (32) or RGBA16F (64) |
| HDR with real alpha or negative values | none among these | HDR profile if it can hold them | none | RGBA16F (64) |

The colour space (sRGB or linear) must be what it is today.

**Ruled out:**

| Format or idea | Why |
|---|---|
| BC4, BC5, EAC R11 and RG11 | a sample returns constants in the missing channels, where today it returns the source's |
| R8 and RG8 as "narrow uncompressed" fallbacks | the same |
| BC5 normal maps | the shader must rebuild z, and the mean length in alpha is lost |
| Moving a mask into another channel or packing several images into one | moves channels |
| Neural decoding in the shader | section 6 |

BC4 stays out **even where today's readers use only `.r`**, because the
guarantee would then rest on every present and future reader. This costs less
than it sounds: a greyscale mask with opaque alpha can use BC1, which is also
4 bits and keeps grey in all three channels, at lower quality than BC4 would
give. BC5 is 8 bits, the same as BC7.

**A normal map is not a 4-bit candidate:** its mips store the mean normal's
length in alpha, which is below 1 wherever normals spread, even when level 0
alpha is 1.

I found no channel swizzle on texture views in the fork's types.

### 3.2 Declare meaning and quality, not a format

Under the rule the channels are fixed, so the declaration only has to say what
the content is and how much quality to keep:

- **content:** `Color` (sRGB), `Data` (linear), `Normal` (linear, normal mip
  filter) or `Hdr`. It sets colour space and mip filtering;
- **quality tier:** for example `Compact`, `Standard`, `High`;
- **mutability:** `Static` or `Mutable`.

Whether alpha is used is **measured**, not declared: a declaration of "opaque"
that was wrong would break the rule.

**Selection:** from the row of 3.1 that matches the measured texels, order the
candidates by tier; drop those whose device feature is absent
(`TEXTURE_COMPRESSION_BC`, `_ETC2`, `_ASTC`, `_ASTC_HDR`), that no installed
encoder produces, or whose block size the base level does not fit; take the
first; else the uncompressed fallback, **reported**. wgpu requires the base
level to be a multiple of the block size. Do not pad or resize to force a fit:
padding moves UVs, and resizing changes what is sampled. Engine devices
request only BC today. The table can mix these formats.

**A tier is not a promised error figure.** An optional step can measure the
result and move up a format when it misses a threshold, at the cost of a
second encode and a measure. This should be the default for 4-bit results.

### 3.3 The field and the API under it

Names are proposals.

```rust
/// The core. A material binder builds a request at run time from what the
/// graph does with each image; the derive supplies a constant one.
pub struct TextureRequest {
    pub content: TextureContent,   // Color, Data, Normal, Hdr
    pub quality: QualityTier,
    pub mutability: Mutability,
}
// TextureStore::acquire(source: &dyn TextureSource, request) -> TextureLease
```

```rust
// Shorthand for a component's image field.
#[gpu(texture(content = Color, quality = Standard))]
pub albedo: TexturePath, // authored and saved as today
```

The GPU side of that field is **a `u32` slot and nothing else**. Under the
rule:

- **For an existing reader, the field must not add or move a column.** The
  slot goes where the reader already reads it. Decals and sprites keep their
  slot inside hand-written packed rows, so either the derive must be able to
  place the slot at that position in a packed record, with the row's layout
  test unchanged, or the existing row writer asks the lease for the slot and
  the declaration only drives loading, encoding and lifetime. A migration that
  cannot do either is not done.
- For a new component, a `u32` column is simply a `u32` the shader reads.
- The CPU declaration and saved data stay as authored (a path or asset
  reference). The trait bound is on that type.
- The dispatch writes the slot already held for (content, request) or the
  row's "nothing" value, and does no other work. A SceneDB-owned binding
  encodes or reads the cache when the pump delivers texels, stages the upload,
  and writes the slot when the texture is whole. Removal releases the lease.

Consumers keep binding the table and sampling as now. SceneDB writes no WGSL.

What SceneDB lacks: content-keyed deduplication and leases in `TextureStore`,
a cache key for generated or multi-source data, selection, and an encoder
interface. Moving residency into SceneDB would **delete** the static tables and
slot plumbing in `helio_component`. Images stay core, but lower than Helio's
`plugin-migration.md` records, which would need updating.

### 3.4 Encoding

| Where | Cost and limits | State |
|---|---|---|
| Build or package time | no runtime cost; one variant per target family adds size; other devices need sources or uncompressed | needs the cook graph, design only |
| First load on a worker, cached | first load pays decode, resize, mips, encode; later loads read | what happens today for BC7 |
| Compute shader at upload | none exists; compute writes blocks to a buffer, then `copy_buffer_to_texture` | unbuilt, unmeasured |

`intel_tex_2` has CPU encoder modules for BC1, BC3, BC4, BC5, BC6H, BC7, ASTC
and ETC1. I saw none for ETC2 RGBA or EAC; ETC1 blocks are valid ETC2 RGB. Its
platform coverage was not checked. No encode timings are recorded in the
repository (`helio_component/tests/real_texture_cook.rs` prints them; not run).
Today a texture whose cook is neither resident nor cached is cooked on the
calling thread; the new path must never do that on the frame thread.

SceneDB is standalone and should not carry Intel kernels in its core: it
defines the encoder interface, the host installs implementations (as it
installs the VRAM owner hook), and a default CPU encoder can be a separate
crate taken from `cooked_texture`. With none installed, everything stays
uncompressed and says so. Mip filtering follows `content`.

The cache key is source content, request, whether alpha is used, target
format, and encoder version.

| Data changed or made at run time | Policy |
|---|---|
| Static, from a file | encode on a worker; cached |
| Generated once | the same, keyed by content; fallback, or upload uncompressed then swap |
| Edited now and then | `Mutable`: uncompressed while changing, re-encoded when settled, swapped. An edit that adds alpha changes which formats qualify |
| Changed every frame | uncompressed |
| Written by the GPU | uncompressed; cannot opt in |
| Must be read exactly (ids, lookup tables, physics data) | uncompressed; these formats are lossy |

Re-encode from the uncompressed source, never from decoded blocks. A swap
holds both copies. A replaced slot can change a texture's size or mip count,
as the detail bias does today.

### 3.5 What it saves

Against today's all-BC7, **only textures that qualify for a 4-bit format save
video memory**, by half: alpha 1 at every mip, and acceptable BC1 quality. If
all 2.04 GiB qualified the saving would be about 1.0 GiB. That is a ceiling
that will not be reached: normal maps and anything using alpha are excluded,
and BC1 is visibly worse than BC7 on some content. HDR images, and textures
that are RGBA8 today, can gain more. **Measure before building** (phase M1).

### 3.6 What fails at build time

| Misuse | How it fails |
|---|---|
| Unknown or malformed `#[gpu]` option | macro error. **Silently ignored today** |
| `texture(...)` on a type that is not an image reference (a number, `Vec`, `GpuHeavy`, a struct), which includes any attempt to compress buffer data | trait bound in generated code, saying hardware formats apply only to textures |
| `texture(...)` with `heavy`, `content_id`, a `mirror` mode, or a `buffer` key shared with a non-texture column | macro error (`mirror = Once` would forbid the later slot write) |
| `texture(...)` in a packed record, until in-place slot placement exists | macro error |
| Unknown content or tier | macro error. There is no way to name a channel-dropping format |
| `#[engine_class]` struct with a texture field and no `scene_store` | macro error, as for `Vec` now |

I saw no compile-fail tests among SceneDB's tests; add them.

Only known at load, so fall back and raise a finding or report, never
silently: the device lacks a format; no encoder; the size does not fit the
blocks; HDR values the format cannot hold; the table is full. That alpha is 1
everywhere is established by the encoder from the texels.

## 4. Buffer data

Block formats do not apply to buffers. There are two separate things here:
what SceneDB may do, which never changes an element, and what a data's owner
may do with its own layout.

### 4.1 What SceneDB (and memory management) may do

| Measure | What it saves | Notes |
|---|---|---|
| Lossless storage compression undone **before upload** (or by a GPU decompression pass that writes the same plain buffer) | disk, package, staging; transfer only with GPU decode. **No resident VRAM** | the pak already does zstd |
| Pool capacity: less headroom on growth, trimming after unloads | resident VRAM | `shrink_to_fit` exists on `DynamicGpuBuffer`; I found no use by `VarLenGpuPool`. A resize rebinds through the epoch, as growth does today |
| Compacting holes | resident VRAM | element types and read code are unchanged, but **offset values change**: handle columns, every row carrying an offset (such as object rows' index ranges), leases and acceleration structures must be updated together. The riskiest item here |
| One pool per device instead of per scene, if duplication is confirmed | resident VRAM | `MEMORY.md` names it as needing a device-level pool registry in SceneDB |
| Releasing data no reader uses | resident VRAM | virtual-geometry meshes already drop most classic geometry (`MEMORY.md`) |
| Allocator block settings | reserved but free driver memory | wgpu's allocator; not investigated |

How much of the 442 MiB of unused pool space or the 799 MiB of free blocks
these could return was not established.

### 4.2 Owner packing: the rules

An owner that packs declares a smaller element type. To SceneDB it is just a
`Pod` element: it uploads the bytes, and the reader binds the same kind of
storage or vertex buffer. SceneDB needs no change and learns nothing.

Where the contract lives, by Helio's invariants:

- **The consumer's api crate owns the row ABI**: the Rust element type, its
  buffer or pool contract, the CPU pack and unpack functions, any fixed vertex
  attribute layout, and **one WGSL unpack snippet** (`//!use ...`).
- **Every shader reader includes that snippet.** None repeats offsets, strides
  or bit layouts. `cargo test -p one_owner` exists to catch WGSL and key
  copies.
- **Layout tests on both sides:** the writer's component asserts its element
  matches the contract; the api crate tests Rust size and every field offset
  against the WGSL, and a GPU round trip of CPU pack to WGSL unpack within a
  stated precision.
- **Other plugins** reach the data only through that api crate, as a Cargo
  edge, with the provider's `buffer` and `api` contracts in their
  descriptors. The api crate holds no `static` or `inventory` collection.
- A layout change is a contract change: every writer and reader changes in one
  piece, and the old path is deleted.

For the mesh vertex pool that crate is `helio-mesh-api` (it already has
`PackedVertex` and `MESH_VERTICES`), and the pair is `pulsar.meshes`.

### 4.3 The mesh vertex pool: who reads it, and what packing asks of each

`builtin_mesh_vertex` holds `PackedVertex` (40 bytes). The static and skeletal
mesh components mirror a list of `PackedVertex` into it (a `Vec` or shared slice)
(`#[gpu(buffer = "builtin_mesh_vertex", mirror = Once, content_id = ...)]`),
so both kinds share one element type. A search for the pool's contract and key
found readers only inside the meshes plugin.

**Fixed-function readers.** These accept only certain formats.

| Reader | Today | What packing asks |
|---|---|---|
| Raster vertex fetch: G-buffer, forward, transparent, shadow, transmittance, selection | `set_vertex_buffer`; each writes its own `VertexBufferLayout`, five with a literal stride of 40 and selection with `size_of` | one layout definition in the api crate, used by all six. Fetch converts only wgpu vertex formats (for example `Float16x2`, which leaves the shader input a `vec2<f32>`); any other packing must arrive as integers and be unpacked in the vertex shader by the owner's snippet. Per-mesh scale and offset, if positions are quantized, are shader code |
| Acceleration-structure build, host path (`host/src/ray_tracing.rs`, static shadow casters) | passes the pool buffer with `first_vertex` = the pool offset and stride `size_of::<PackedVertex>()` to Helio | see below |
| Acceleration-structure build, component path (`component/src/ray_scene.rs`) | the same inputs | see below |

Both builds go through `helio_core::acceleration`, which declares
**`Float32x3`** positions at the start of each stride and `Uint32` indices.
The pool has no `BLAS_INPUT` usage, so Helio copies the raw byte range into a
build input; the copy converts nothing. So packing must either **keep f32
positions first in the element** (nothing here changes but the stride), or:

- use a format the fork allows only with
  `EXTENDED_ACCELERATION_STRUCTURE_VERTEX_FORMATS` (Vulkan, DX12):
  `Float32x2`, `Float16x2/x4`, `Snorm16x2/x4`. I found no request for that
  feature, so it also needs a fallback where it is missing;
- change Helio's build to take the format from its caller (a kind-neutral
  parameter);
- for normalized positions, apply each mesh's scale and offset through a build
  transform (`transform_buffer`, unused today) or the instance transform.
  Neither was checked.

**Shader readers** (storage, as `array<u32>`).

| Reader | Today | What packing asks |
|---|---|---|
| `mesh_rays.wgsl`, `mesh_hit_surface.wgsl` | include `//!use mesh_ray_surface` (`helio-mesh-api/src/shaders/ray_surface.wgsl`), whose `mesh_vertex_*` functions decode with `MESH_VERTEX_WORDS = 10`; I found no direct indexing of `vertices` in the two shaders | change the snippet |
| `component/src/skin_cache.wgsl` | **its own copy of the decode** (`PACKED_VERTEX_WORDS = 10`, normal and tangent at words 8 and 9) | first move the vertex decode into one api snippet that both include (it is a second copy today, whatever happens to packing). Its output, the skinned vertex, is a separate contract and need not change |

**CPU readers and writers.** The pool element is the component field's element,
and SceneDB uploads that slice as given. So packing the pool means the owner
either changes the CPU field to the packed type, or keeps a separate packed
copy to mirror (more CPU memory).

| Code | Role | What packing asks |
|---|---|---|
| `component/src/mesh_spatial.rs` (picking BVH), `editor/src/thumbnails.rs` | read `.position` | CPU unpack from the api crate, or full-precision data |
| `component/src/mesh_cache.rs` | serializes `PackedVertex` into the native mesh format and sizes by it | a new format version, or packing at load |
| Host ray tracing's change detection | hashes the CPU vertex bytes | works on any bytes |
| `component/src/virtual_geometry_host.rs` | builds cluster data from CPU vertices; writes fallback geometry into the pool; sizes its memory report by the type | unpack for the builder; pack the fallback |
| `component/src/authored_lod.rs` | forwards the pool buffer handle | nothing |
| `asset-compat` converters, the level editor's default-level geometry, examples and tests across many plugins | construct `PackedVertex` | construct through the api crate's pack function; struct literals must change |
| `component/src/skin_cache.rs` test, `api/tests/wgsl_rows.rs` | assert 40 bytes and the WGSL word count | become the contract's layout tests, extended to field offsets |

Not readers of this pool: the ray proxy pool (its own five-float layout),
virtual-geometry cluster pages (their own format), and the bake plugin's
Nebula shaders (their own vertex struct). SceneDB's `gpu/assets.rs` also
defines a key named `builtin_mesh_vertex` for its own geometry arena; whether
anything live uses it was not checked, and under rule R SceneDB should define
no mesh layout.

**What it could save** (my arithmetic, not a design): keeping f32 positions,
half-float UVs and the bitangent sign folded into the tangent give 28 bytes
against 40 (30% less); 8-byte positions give 24 (40% less). On about 498 MiB
live that is roughly 150 to 200 MiB, and *allocated* memory falls only if pool
capacity follows. Half-float precision for tiled and lightmap UVs must be
checked per mesh. Keeping f32 positions avoids every acceleration-structure
question, so it is where a meshes plan should start.

**Index pool.** Raster draws, both acceleration-structure builds (`Uint32`
declared, copied at 4 bytes an index) and shaders reading `array<u32>` depend
on it. 16-bit indices would be the owner's choice under the same rules, but a
draw has one index format, so mixing needs separate pools. Its readers were not
traced individually.

## 5. Compression kinds in SceneDB

The user asked for an enum of the kinds available, since SceneDB has none.
Under the rule it is small. Storage and resident form are **two independent
settings**: one texture can be zstd on disk and BC7 in VRAM at once, so a
single flat list would not work. Names are proposals.

```rust
/// How bytes are kept while no reader needs them. Closed. Lossless and
/// byte-exact; always undone before a GPU reader can see the data.
pub enum StorageCodec {
    Raw,
    Lz4,   // a candidate: keep only if measurements justify it
    Zstd,
}

/// What a resident resource holds.
pub enum ResidentForm {
    /// The declared element or an uncompressed texel format. The only form a
    /// buffer can have.
    Plain,
    /// Textures only: a format the sampler decodes. wgpu's enum, not a copy.
    HardwareBlock(wgpu::TextureFormat),
}
```

There is no shader-decoded variant and no registry of buffer layouts. Buffer
packing is not a SceneDB kind: it is the owner's row contract (4.2).

- **What data declares:** textures, a `TextureRequest` (3.2), from which the
  form is chosen; stored payloads, a codec chosen by the writer and recorded
  in the payload. In `derive_cached` files the 8 reserved header bytes can hold
  codec and raw length, with old zeros read as `Raw`. Buffers declare nothing
  to SceneDB.
- **One owner:** `pulsar_content`'s private `Codec { Raw, Zstd }` covers two
  of these. Share one definition (in SceneDB, or a small shared crate) and keep
  the pak's byte ids. Do not compress twice.
- **Decoders and missing hardware:** storage codecs are compiled into SceneDB
  with a CPU decode always present; an unknown id is an error naming it. A GPU
  decompression path would only be an accelerator. Hardware formats are
  decoded by the sampler; without the feature, another variant or
  uncompressed.
- **Plugins:** they add no SceneDB kinds. Storage codecs are closed so saved bytes
  stay readable whatever a project enables; texture formats are wgpu's. A
  plugin can supply an **encoder**, or a producer that reads its own file and
  hands SceneDB an ordinary texture. SceneDB sees an interface and names no
  plugin.

### 5.1 Reporting compressed against raw

SceneDB's `VramUse` has a label, capacity and live bytes; the engine's
`VramClaim` adds an owner. `TextureStore` reports no claim, and all material
textures share one label. Add to a claim an optional report: the form,
**live resident bytes** and **live uncompressed-equivalent bytes** (for
textures, the same mips in the fallback format of 3.1).

- The equivalent is what the data *would* take, not a measured saving.
- It covers live bytes only. Slack stays `capacity - used`, so a ratio cannot
  hide pool slack.
- Unknown stays unknown.
- For a pool its owner packed, SceneDB sees only the element. The owner can
  report the saving through its own existing claim
  (`VramProviderRegistration`); SceneDB needs no packing kind for it.
- Stored sizes are reported where they happen (derived-cache counters; the pak
  writer's `written_raw` and `written_stored`).

`TextureStore` holds each texture, so it can compute both. Showing the format
per allocation needs it in the label, which would split the one "x565" group:
a VRAM Map design choice. Either would have settled the question in 1.0.

## 6. Neural texture compression under the rule

Neural texture compression stores a material's texture set as small latent
textures plus a small network that turns them back into all the channels. It is
lossy, trained per set, and gains from correlation between channels.

| Mode | Under the rule | Video memory against today |
|---|---|---|
| Decode **on sample** (network in the shader) | **ruled out**: the shader changes | would save |
| Shader-sampled neural sets of any other design, including filtered BC latents | **ruled out** | would save |
| Decode **on feedback** (tiles, through a page table) | **ruled out**: software virtual texturing adds a page lookup to every sample. Hardware sparse textures and sampler feedback are not exposed, and would still need the shader to handle a miss | would save |
| Decode **on load** into ordinary block textures | allowed: the shader sees a normal table texture | **none**: the result is BC7 as today |

**So under the rule neural compression cannot save video memory.** Its one
remaining use is shrinking stored and shipped bytes, at the price of a second
lossy step, load time to run the network over every texel of every mip, and a
fast BCn encoder that does not exist here (decoding to RGBA8 instead would
quadruple video memory against BC7). Decoded output must also keep all
channels, including normal-spread alpha.

**Recommendation: do not build it now.** Packages today hold source images in a
zstd pak. First ship cooked textures and measure. If shipped size is then still
a problem, compare neural on-load against rate-distortion-aware BCn plus the
pak's zstd, which needs no network at load. Only if neural wins clearly should
it exist, as an optional plugin (`pulsar.neural_textures`) that is a
**producer**: it reads its own asset and hands the existing texture path an
ordinary block texture. It needs no change to material shaders, `helio-mats`,
Helio core or SceneDB's kinds. Disabled, materials load their source images.
Sources stay the truth; a stale asset falls back and raises a finding;
compression is an editor task. It does not belong in the meshes plugin, which
neither owns nor is the only user of material textures.

Licence and patents would need review before using any vendor SDK: NVIDIA's is
a beta under its RTX SDKs licence and compresses with CUDA on NVIDIA GPUs.

### 6.1 Published and shipping work, for the record

Checked on 2026-11-27 from the sources in 13. Numbers are the authors' own;
none were reproduced. Their video-memory results come from modes ruled out
here.

| Work | Design and result |
|---|---|
| NVIDIA research (2023) | latent grids, two hidden layers of 64, stochastic filtering; project page: 16x the texels of high-quality BC with 30% less memory; a 9-channel 4K set compresses in 1 to 15 minutes on an RTX 4090 |
| NVIDIA RTXNTC SDK 0.10.1 beta | on load, feedback (DX12 only) or sample; about 5 bits a texel for 40 to 50 dB PSNR; 2K example: on load 2.5 MB on disk but 12 MB in VRAM, the same VRAM as BCn |
| Ubisoft La Forge (2024) | BC6 features, 16-wide hidden layer, hardware filtering; added 0.64 ms on a full-screen 4K quad on an RTX 2070 |
| *Assassin's Creed Mirage* (Ubisoft article, February 2026) | shipped, decoded in the pixel or G-buffer shader; about 30% saving, on a subset of assets because of runtime cost |
| Intel TSNC (April 2026, trade press) | BC1 latent pyramid; 9x to 17x; reports disagree on whether the SDK is released |

**Compared with these, this plan saves less video memory on neural content, by
choice:** it keeps the generic read.

## 7. Ownership

| Part | Owner |
|---|---|
| Texture request, selection, leases and deduplication, upload, encoder interface, `StorageCodec`, storage decoders, `ResidentForm`, size reporting | SceneDB (codec ids shared with the pak) |
| Encoder implementations and mip filters | a default encoder crate, installed by the host |
| Device features and settings | the engine (`render_runtimes/device.rs`) |
| Material textures requesting through the runtime API | `pulsar.materials` |
| Component image fields opting in | each kind's plugin |
| Row layouts, table bindings and sampling | consumers, **unchanged** |
| Pool growth, trimming, compaction, sharing | SceneDB, with the owners of rows that carry offsets |
| Packing any buffer data: element type, pack and unpack, vertex layout, WGSL snippet, tests | the owning writer and reader pair, with the contract in the consumer's api crate (`helio-mesh-api` for mesh pools) |
| Taking a position format in acceleration-structure builds, if an owner needs it | Helio core, as a kind-neutral parameter |
| An optional neural producer | its own plugin, if ever |

Rejected: a SceneDB macro, kind or registry that packs buffers (rule R); any
codec in Helio core (it knows no asset format); texture compression owned by
the meshes plugin.

## 8. How others do it, and what scales with what

From general knowledge, not re-checked: Unreal gives each texture asset a
compression setting and cooks a format per platform; Unity does the same with
per-platform import settings; Basis Universal and KTX2 ship one intermediate
form and transcode it to the device's family. Rate-distortion-aware BCn with a
general compressor shrinks packages (Oodle Texture's documentation says it
reduces distribution size, not GPU size).

This design is the first approach, declared on a field or by a material
binder, encoding at first load, which suits an editor that is also the runtime.
For textures it is **more restricted** than those engines, which use single-
and two-channel formats with matching shader code; the rule trades that memory
for unchanged readers. Compact vertex formats are common in engines, and here
they are the owner's job (4.2). A shipped game should move encoding to the package step.

| Cost | Scales with |
|---|---|
| Encoding | texels of each distinct (content, request) pair, once per change |
| Video memory | resident textures x bits per texel: a constant factor, not a bound |
| Per frame | only what changed; selection is constant time |
| Storage decode | bytes loaded, off the frame thread |

Because it is a constant factor, **per-texture mip streaming** matters more in
the long run: replacing a slot's texture with more or fewer mips by screen
size is invisible to read code, as today's global detail bias is. Software
virtual texturing is ruled out (section 6's reason).

## 9. Phases

Every SceneDB phase must pass the proof in R; V is an owner's contract change
and passes 4.2's tests.

| Phase | Work | Done when |
|---|---|---|
| **H0** | Make bad `#[gpu]` attributes errors; compile-fail tests | a typo fails the build; existing uses build |
| **M1. Measure** | `TextureStore` claims and size reporting in the VRAM Map; a census of real textures: alpha 1 at every mip, BC1 quality on lit frames | the 1.0 format question answered from the map; a measured saving; go or no-go for H3 |
| **H2** | Runtime API (request, selection, encoder interface, leases); move residency out of `helio_component`; materials use it; formats unchanged | identical textures on the same content; old statics and plumbing deleted; no WGSL or layout change |
| **H3** | 4-bit formats for qualifying textures; request every family the adapter has; other variants if a target needs them | four-channel quality tests at every mip; lit-frame comparison; measured saving |
| **H4** | The `texture(...)` field; move component images to it | hand-written slot code deleted, row layout tests unchanged |
| **H5** | `Mutable` textures; a compute encoder only if measured need | its own measurements |
| **S** | Shared storage codec types; codec id in derived-cache files | old caches and paks read; measured size and load time decide defaults |
| **P** (independent) | Cooked textures in the package (needs the cook graph) | packaged games load cooked textures; sizes measured |
| **V** (independent, `pulsar.meshes`, only if wanted) | A meshes plan for a packed vertex: first one vertex layout definition and one WGSL decode snippet in `helio-mesh-api` (removing the skinning copy), then the layout | 4.2's tests; every reader in 4.3; raster, ray, skinning, picking and file loading verified; saving measured |
| **Mem** (independent, not compression) | Why pools are slack; growth and trim policy; per-scene duplication; compaction only if still worth it; driver block use | each measured on the VRAM Map |

If M1 shows little qualifies for 4 bits, stop after H0 and M1: H3 would not
pay, and H2 and H4 would then be cleanup to judge on their own merits. For
video memory, **Mem** may well be worth more than H3.

## 10. Risks

| Risk | Mitigation |
|---|---|
| Little qualifies for 4 bits, so VRAM barely moves | M1 first |
| BC1 quality is poor on some content | per-texture measure with move-up; tiers |
| A field opt-in silently changes a row layout | the rule's proof: layout tests and WGSL unchanged |
| Alpha appears after encoding (edits, reloads) | alpha is re-measured on every encode |
| Moving residency into SceneDB breaks texture behaviour | H2 changes no format and compares textures |
| An owner's packed layout is decoded in more than one place | one snippet and one layout definition in the api crate, before any packing; `one_owner` |
| Packed positions break ray tracing | keep f32 positions first unless the whole path in 4.3 is built and has a fallback |
| Compaction leaves a stale offset | do it last, if at all; update every holder together |
| Encode cost slows first load | cached, off the frame thread; package-time encode later |

## 11. Decisions for the user, and follow-ups

1. **Textures and ownership.** The packing rule was given for buffer data.
   May an owner likewise choose formats or decoding for textures it both
   writes and reads (such as BC4 or BC5, or neural decoding in the materials'
   shader), under the same contract rules? Until you say so, this plan keeps
   the strict channel rule: it excludes BC4 even where current readers use
   only `.r` (3.1), and rules neural decoding in shaders out (6).
2. **Neural compression:** agree to set it aside (section 6)?
3. What matters first: video memory (M1, Mem, V) or package size (P, S)?
4. Accept moving texture residency into SceneDB (3.3).
5. Are targets beyond BC hardware needed now?
6. Quality thresholds for 4-bit results.
7. Is a packed mesh vertex (V) wanted at all?

Follow-ups outside this plan: verify and fix or correct the documentation of
`streaming.texture_stream_pool_mb`; per-texture mip streaming.

## 12. Not verified

- Nothing was built or run; I measured no ratio, quality, memory saving or
  timing for Pulsar content. The VRAM figures are read from the screenshot.
- The screenshot's formats: no allocation states one; the project was not
  identified, no log read, one derived cache's headers read, tiles not counted.
- No census: how many textures have alpha 1 at every mip, or survive BC1, is
  unknown.
- Readers in 1.4 come from searches and partial reading of the shaders. The UI
  shader's later use of its image, the material preview and graph-generated
  samples were not traced in full. Textures not bound through the table were
  not traced.
- The channel behaviour of block formats, bits per texel, and ASTC, ETC2 and
  EAC capabilities are general knowledge, not tested on a device. A
  four-channel format can still bleed error between channels; only tests will
  show whether data textures tolerate it.
- No macro change was prototyped. The silent-ignore behaviour was read, not
  reproduced. In-place slot placement in packed records is untested.
- No encoder was run or timed; `intel_tex_2`'s targets were not checked.
- The vertex pool readers in 4.3 come from searches for the contract, key and
  type, and reading the parts they pointed to, not from a full trace. Index
  pool readers were not traced one by one. Whether the bake host reads mesh
  vertices from components was not settled; build transforms and extended
  position formats were not tried. No packed layout was built or measured.
- Compaction's holders of offsets were not enumerated; allocator settings were
  not investigated.
- The tier-budget finding comes from reading and searching, not a measurement.
- The wgpu feature checks were searches at one checkout (`0883b3b`).
- Published figures reached me through automated fetch summaries. The 2023
  paper's results, the RTXNTC library docs and Intel's own pages were not read;
  patents were not searched.
- Other work had uncommitted changes in several submodules and `crates/core`
  files while this was written; the audit describes the working tree as read.

## 13. Sources

- [RTXNTC SDK](https://github.com/NVIDIA-RTX/RTXNTC), its [renderer notes](https://github.com/NVIDIA-RTX/RTXNTC/blob/main/docs/Renderer.md) and [licence](https://github.com/NVIDIA-RTX/RTXNTC/blob/main/LICENSE.txt); [RTXNTC library](https://github.com/NVIDIA-RTX/RTXNTC-Library)
- [Random-Access Neural Compression of Material Textures](https://research.nvidia.com/labs/rtr/neural_texture_compression/) ([arXiv](https://arxiv.org/html/2305.17105v1))
- [Real-Time Neural Materials using Block-Compressed Features](https://arxiv.org/html/2311.16121v2)
- [Shipping Neural Texture Compression in Assassin's Creed Mirage](https://www.ubisoft.com/en-us/studio/laforge/news/415slmB3ZGzv8d2LhO9QT/shipping-neural-texture-compression-in-assassins-creed-mirage)
- [Intel TSNC report](https://itbrief.com.au/story/intel-releases-neural-texture-compression-sdk-for-game-devs)
- [Oodle Texture in Unreal Engine](https://dev.epicgames.com/documentation/unreal-engine/oodle-texture-in-unreal-engine)

## 14. Report (Helio architecture discipline)

- **Design:** a proposal only, under rule R. SceneDB gains a texture request
  API with a field shorthand, hardware formats that keep all channels,
  installed encoders, lossless storage codecs and size reporting. Its changes
  require nothing of consumers. Buffer packing belongs to the writer and
  reader's owner, with the contract in the consumer's api crate. No neural
  work is recommended now.
- **AAA comparison:** section 8. More restricted than the engines compared, by
  the rule; scaling in section 8.
- **Deleted:** no code. From the earlier draft of this document: shader-decoded
  kinds, the neural on-sample path and its material seam, single- and
  two-channel texture formats. Buffer packing left SceneDB for the data's owner.
- **Verified:** claims about Pulsar come from reading and searching code and
  docs. No gate, test, frame or A/B was run, since no code changed. Gaps are in
  section 12.
