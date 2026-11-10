//! Small field and item attribute checks shared by the macros.
use syn::{Attribute, Field, punctuated::Punctuated};

/// Check whether a type already derives a specific trait by final segment ident.
pub(crate) fn has_derive(attrs: &[Attribute], trait_ident: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }

        attr.parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
            .map(|paths| {
                paths.iter().any(|p| {
                    p.segments
                        .last()
                        .map(|s| s.ident == trait_ident)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    })
}

pub(crate) fn has_sub_props_attr(field: &Field) -> bool {
    field
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("sub_props"))
}

// ── Auto-derived GPU mirroring (Pulsar-Native#561) ──────────────────────────
//
// `#[gpu]` on a `#[property]` field opts it into an auto-generated, `Pod`,
// SceneDB-mirrored companion component -- the pattern `LightComponent`'s own
// hand-written `LightGpuData`/`LightGpuRow` companion (`helio_component`)
// proved out by hand before this generator existed; `LightComponent` itself
// has since been normalized onto this exact generated path (its
// `LightComponentGpuMirror`), with no hand-written companion of its own left
// at all. See `gpu_mirror_codegen`'s doc for the composed-struct shape and
// `pulsar_world_registry::GpuMirrored`'s doc for the runtime contract this
// generates an impl of.
//
// Bare `#[gpu]` mirrors a field as its own exact bytes (any `Copy` type, via
// `GpuRepr<T>` -- no classification, see that type's own doc). When the GPU
// consumer's own protocol genuinely differs from the property's edit-time
// shape -- different units, an enum value the consumer has no equivalent
// for -- `#[gpu(as = Type, with = path::to::fn)]` computes that difference
// ONCE, at mirror-build time, instead of a hand-written function downstream
// of the mirror. See [`GpuFieldOverride`]'s own doc for the full design,
// when to reach for it, and the real performance trade it makes (a wider
// GPU-side type costs more storage/bandwidth per row) against the ones it
// doesn't (it never runs per rendered frame, in any `#[gpu]` mirror mode).
