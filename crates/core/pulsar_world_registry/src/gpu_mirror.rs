//! The GPU companion layout derived for `#[gpu]` fields: `GpuMirrored`, `GpuRepr` and `NoGpuMirror`.

// ── Auto-derived GPU mirroring ──────────────────────────
//
// `LightComponent`'s original hand-written `LightGpuData`/`LightGpuRow`
// companion (`helio_component`) proved the pattern by hand: a second,
// `#[gpu]`-mirrored SceneDB component holding only a type's render-relevant,
// `Pod`-safe translation, kept in step by hydrate, so a renderer never has
// to hand-roll a Helio-side cache/sync-by-diff for that data again. This
// section is the same pattern generated automatically by `engine_class_
// derive` for any `#[property]` field marked `#[gpu]` -- `LightComponent`
// itself has since been normalized onto it (its `LightComponentGpuMirror`),
// with no hand-written companion left. See `GpuRepr<T>`'s doc below for the
// (universal -- any `Copy` type mirrors as its own exact bytes, no
// allowlist/denylist/semantic conversion) packing rule, and `engine_class_
// derive`'s own doc for how `#[sub_props]` nesting composes.
//
// ## The GPU upload modes this covers, and where field-level transforms fit
//
// A `#[gpu]` field's bytes reach a real `wgpu::Buffer` through one of a few
// SceneDB-level upload modes, chosen by the field's own shape (never
// something a component author picks by hand):
//
// - **`DirtyTracked`** (the default for a scalar `#[gpu]` field): keeps a
//   full CPU shadow, re-uploads only the rows that actually changed since
//   the last flush. `to_gpu_mirror()` -- and therefore any `engine_class_
//   derive::GpuFieldOverride` (`#[gpu(as = .., with = ..)]`) on that field
//   -- runs once per genuine property edit, never per frame.
// - **`Once`**: uploaded a single time, at first insert, never re-run
//   after. Cheaper still, same "not per frame" property.
//
// Var-len (`Vec<T>`) and heavy handle/heavy-element storage shapes are
// SceneDB's own `#[derive(SceneStore)]` concern exclusively and are NOT
// routed through this crate's companions.
//
// `#[gpu(as = Type, with = path)]` (`engine_class_derive::GpuFieldOverride`)
// only touches the FIRST of these today (plain scalar `DirtyTracked`/`Once`
// fields) -- it changes what bytes a field's `to_gpu_mirror()` call
// produces, not which of these upload paths carries them. Its performance
// story rides entirely on the mode it's used in: since none of the above
// modes re-run `to_gpu_mirror` per rendered frame, neither does `with` --
// its cost is bounded by edit frequency, not frame rate, regardless of
// which mode the field is on. See `GpuFieldOverride`'s own doc
// (`engine_class_derive`) for the full design and the one real cost it DOES
// have (a wider `as` target type costs more GPU storage/bandwidth per row).

/// A type whose `#[gpu]`-marked `#[property]` fields (`engine_class_derive`)
/// have an automatically-derived, `Pod`, SceneDB-mirrorable translation.
///
/// The translation is never stored as a component. `engine_class_derive`
/// registers a SceneDB GPU dispatch for the authored type that calls
/// [`Self::to_gpu_mirror`] and writes the companion's packed row
/// (`pulsar_scenedb::gpu::write_derived_row`), so the row follows the
/// authored value through every SceneDB write, removal and mirror replay.
///
/// `engine_class_derive` generates an impl of this for EVERY
/// `#[engine_class(...)]`-processed struct, unconditionally -- including
/// ones with zero `#[gpu]` fields, which get `GpuMirror = NoGpuMirror` (see
/// that type's doc for why this uniform generation, rather than only
/// generating an impl when there's something real to mirror, is what makes
/// `#[sub_props]` composition possible with no special-casing at each
/// nesting level: a containing struct's own generated `to_gpu_mirror` reads
/// `<SubPropsTy as GpuMirrored>::GpuMirror`/`to_gpu_mirror()` for every
/// `#[sub_props]` field unconditionally, and that has to type-check whether
/// or not that particular sub-props group happens to contain any `#[gpu]`
/// leaves this time).
pub trait GpuMirrored {
    /// The `Pod` GPU-mirror type. `NoGpuMirror` when there's nothing to
    /// mirror (no `#[gpu]` fields anywhere in this struct or its
    /// `#[sub_props]`).
    type GpuMirror: pulsar_scenedb::Pod + Send + Sync + 'static;

    /// Translate `self`'s current `#[gpu]`-marked fields (and its
    /// `#[sub_props]` fields' own translations) into `Self::GpuMirror`.
    fn to_gpu_mirror(&self) -> Self::GpuMirror;
}

/// Wraps ANY `Copy` type for GPU mirroring, treating its exact Rust memory
/// representation as the GPU-side bytes -- no classification, no allowlist
/// of "recognized" types, no semantic conversion (a `bool` stays its own
/// 1-byte representation; an enum stays whatever bytes its own `#[repr]`
/// gives it). This is the ONLY thing standing between an arbitrary
/// `#[gpu]`-marked field and SceneDB's `Pod` bound -- see [`Self`]'s safety
/// note below for exactly what it does and doesn't guarantee.
///
/// # Why this exists (the auto-mirror generator's field
/// support must be universal, not a hand-maintained list of "supported"
/// shapes)
///
/// `pulsar_scenedb::Pod`'s own safety contract is stricter than this needs:
/// it requires all-zero bytes (and by extension, in how it's actually used
/// elsewhere in SceneDB, ANY byte pattern) to be a valid value of `T`,
/// because SOME of its use sites hand out `&[T]` over raw, possibly-
/// zeroed-or-arbitrary memory and let safe code read it back directly. A
/// `#[gpu]`-mirrored component field is not that use site: `World::insert`
/// only ever writes bytes copied out of an ALREADY-VALID, live `T` the
/// caller is holding -- there is no "hand out uninitialized memory as
/// `&Self`" step anywhere in that path. What crosses to the GPU is real
/// data, once; what happens to those bytes after that (a shader reads
/// them, a compute pass writes new ones back) is the consumer's problem to
/// interpret correctly, not something the Rust type system can, or needs
/// to, verify on the way there.
///
/// # Safety (for the blanket `unsafe impl Pod for GpuRepr<T>` below)
///
/// This is a deliberate, narrower contract than `Pod`'s doc literally
/// states -- `GpuRepr<T>` is sound to WRITE (copy an existing `T`'s bytes
/// out) for any `Copy` `T`, unconditionally. It is only sound to READ BACK
/// arbitrary bytes as a live `T` (e.g. `World::get`, `readback_row` after a
/// compute shader wrote something) if whatever produced those bytes
/// produced a valid `T` in the first place -- exactly as true of any GPU
/// buffer read in any graphics API, and exactly the "bytes are just bytes,
/// the consumer interprets them" contract this type exists to make
/// explicit rather than pretend doesn't apply.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GpuRepr<T: Copy>(pub T);

// SAFETY: see this type's own doc above -- a deliberate, narrower contract
// than a literal reading of `Pod`'s doc comment, scoped to the write-only
// GPU-mirror use case this type exists for.
unsafe impl<T: Copy + Send + Sync + 'static> pulsar_scenedb::Pod for GpuRepr<T> {}

impl<T: Copy> std::ops::Deref for GpuRepr<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Copy> std::ops::DerefMut for GpuRepr<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T: Copy> From<T> for GpuRepr<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

/// The `GpuMirror` for a type with no `#[gpu]`-marked fields anywhere (the
/// common case -- most components have none). Zero-sized, so a struct that
/// composes one in via `#[sub_props]` (`engine_class_derive`'s generated
/// mirror struct embeds `<SubPropsTy as GpuMirrored>::GpuMirror` per
/// sub-props field, unconditionally) pays nothing for a sub-props group
/// that happens to contribute no real data this time.
///
/// Deliberately a dedicated type, not `()`: `()` is a general-purpose Rust
/// primitive with its own broader meaning, and giving "nothing to mirror"
/// its own named type keeps a `GpuMirror = NoGpuMirror` reader unambiguous
/// about which of the trait's two cases they're looking at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoGpuMirror;

// SAFETY: `NoGpuMirror` is a zero-sized unit struct: it has no bytes, so there
// is no padding and every (empty) bit pattern is a valid value.
unsafe impl pulsar_scenedb::Pod for NoGpuMirror {}

// `SceneGpuStore::register_gpu_buffer` keys shared buffers on an element type
// that must be `Reflectable`, so the inspector can describe a buffer's layout.
// `GpuRepr<T>` is a transparent wrapper: it reflects exactly as `T` does.
impl<T> pulsar_reflection::Reflectable for GpuRepr<T>
where
    T: Copy + pulsar_reflection::Reflectable,
{
    fn type_info() -> &'static pulsar_reflection::RuntimeTypeInfo {
        <T as pulsar_reflection::Reflectable>::type_info()
    }

    fn serialize(
        &self,
        serializer: &mut dyn pulsar_reflection::TypeSerializer,
    ) -> pulsar_reflection::ReflectResult<()> {
        <T as pulsar_reflection::Reflectable>::serialize(&self.0, serializer)
    }

    fn deserialize(
        deserializer: &mut dyn pulsar_reflection::TypeDeserializer,
    ) -> pulsar_reflection::ReflectResult<Self> {
        <T as pulsar_reflection::Reflectable>::deserialize(deserializer).map(GpuRepr)
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(*self)
    }
}

static NO_GPU_MIRROR_TYPE_INFO: pulsar_reflection::RuntimeTypeInfo =
    pulsar_reflection::RuntimeTypeInfo {
        type_id: std::any::TypeId::of::<NoGpuMirror>(),
        type_name: "NoGpuMirror",
        size: 0,
        align: 1,
        structure: pulsar_reflection::TypeStructure::Struct { fields: &[] },
        color: None,
    };

impl pulsar_reflection::Reflectable for NoGpuMirror {
    fn type_info() -> &'static pulsar_reflection::RuntimeTypeInfo {
        &NO_GPU_MIRROR_TYPE_INFO
    }

    fn serialize(
        &self,
        _serializer: &mut dyn pulsar_reflection::TypeSerializer,
    ) -> pulsar_reflection::ReflectResult<()> {
        Ok(())
    }

    fn deserialize(
        _deserializer: &mut dyn pulsar_reflection::TypeDeserializer,
    ) -> pulsar_reflection::ReflectResult<Self> {
        Ok(NoGpuMirror)
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(*self)
    }
}
