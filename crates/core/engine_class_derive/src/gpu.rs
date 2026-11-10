//! The `#[gpu]` field attribute and the generated GPU mirror companion.
//! Entry point: [`gpu_mirror_codegen`] (packing rules in its doc).

use proc_macro::TokenStream;
use quote::quote;
use syn::Field;

pub(crate) fn has_gpu_attr(field: &Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("gpu"))
}

/// `#[gpu(as = Type, with = path::to::fn)]` -- an OPTIONAL upload-time
/// transform for a `#[gpu]`-marked leaf field, computed once when the
/// mirror is built (`to_gpu_mirror`), never at properties-panel-edit time.
///
/// Exists so a field's `#[property]`-facing representation (the shape a
/// human edits -- degrees, a business-logic enum with variants the GPU
/// consumer doesn't itself have) can differ from its GPU-mirrored one (the
/// shape the shader/renderer actually wants -- radians, a plain `u32` the
/// consumer already knows how to interpret) WITHOUT a hand-written, post-
/// mirror translation function anywhere: the mirror IS already in the
/// consumer's preferred shape the moment SceneDB builds it. `with` is
/// called exactly once per `to_gpu_mirror()` call (the same call site every
/// other `#[gpu]` field already goes through) -- not a second, separate
/// pass a consumer has to remember to run.
///
/// Both arguments are required together: `as` names the mirror field's
/// resulting type (what `with`'s return type must match -- enforced by
/// ordinary rustc type-checking on the generated `GpuRepr<Type>(path(self.
/// field))` expression, not by this parser), `with` is a plain `fn(FieldTy)
/// -> Type` item path (an inherent method like `f32::to_radians` works
/// directly, no closure wrapper needed). Bare `#[gpu]` (the overwhelming
/// majority of fields) skips this entirely -- the mirror holds the field's
/// own exact type/bytes unchanged, same as before this existed.
///
/// # When to reach for this vs. leaving a field bare `#[gpu]`
///
/// Use `as`/`with` when the GPU consumer's own protocol genuinely differs
/// from the property's edit-time shape: different units (degrees vs.
/// radians), or a value space the editor exposes that the consumer doesn't
/// fully support (an enum variant with no equivalent on the other side --
/// `LightComponent`'s `LightType::Area`, remapped to `Point` for Helio,
/// which has no area lights). Leave a field bare `#[gpu]` whenever the
/// property's own type already IS the byte shape the consumer wants --
/// which is the overwhelming majority of fields; reaching for `with` when
/// a plain `GpuRepr<T>` copy would do is adding an indirection for nothing.
///
/// # Performance, and how it interacts with SceneDB's GPU upload modes
///
/// `with` runs exactly once per [`pulsar_world_registry::GpuMirrored::
/// to_gpu_mirror`] call -- and `to_gpu_mirror` itself only runs when
/// SOMETHING actually changed: a genuine property edit (hydrate) for a
/// `DirtyTracked` field (SceneDB's default `#[gpu]` mirror mode), or once
/// ever, at first insert, for a field mirrored `Once`. Neither mode calls
/// `to_gpu_mirror` per rendered frame -- a consumer wanting the mirrored
/// value every frame reads the already-computed `GpuRepr<Type>` straight
/// out of `World` (a page-column read, not a GPU round trip and not a
/// re-run of `with`), the same as any other `#[gpu]` field. So: `with`'s
/// own cost is bounded by EDIT frequency, not FRAME rate -- a `to_radians()`
/// call or a four-arm `match` is free at that cadence, even during an
/// editor slider drag firing many edits a second. This is exactly the
/// "conversion belongs at upload time, not sprinkled through the render
/// loop" property the whole `#[gpu]` mirror system is built around; `with`
/// doesn't change that property, it just gives a field a way to land in a
/// DIFFERENT byte shape at that same upload-time boundary, instead of only
/// ever being a same-shape copy.
///
/// The one real cost worth knowing about: `as`'s target type decides the
/// mirror field's GPU storage width, and `with` can WIDEN it -- `#[gpu(as =
/// u32, with = ...)]` on a `bool` source (SceneDB's own `cast_shadows`
/// encoding) costs 4 bytes of GPU storage/bandwidth per row instead of the
/// 1 byte a bare `#[gpu] bool` field would (via `GpuRepr<bool>`, `elem_align`
/// -- see that fn's own doc). That's the right trade when the consumer
/// needs a `u32` sentinel directly and would otherwise have to widen it
/// itself on every read; it's a real, non-zero one to be deliberate about,
/// not an argument against using `with` where it's actually needed.
///
/// Not yet wired into any other `#[gpu]` field shape: extending `as`/`with`
/// to var-len list elements is a bounded future addition if a real field
/// needs it -- nothing about this design blocks it, it just hasn't had a
/// real caller yet. (Var-len and heavy storage shapes are SceneDB's own
/// `#[derive(SceneStore)]` concern; this derive classifies packed scalars
/// only.)
pub(crate) struct GpuFieldOverride {
    pub(crate) target_ty: syn::Type,
    pub(crate) with_path: syn::Path,
}

/// Parses a `#[gpu]` field attribute's arguments, if any -- `None` for bare
/// `#[gpu]`, `Some(GpuFieldOverride)` for `#[gpu(as = ..., with = ...)]`.
pub(crate) fn parse_gpu_attr(field: &Field) -> syn::Result<Option<GpuFieldOverride>> {
    let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("gpu")) else {
        return Ok(None);
    };
    if matches!(attr.meta, syn::Meta::Path(_)) {
        return Ok(None); // bare `#[gpu]`, no override
    }
    let mut target_ty: Option<syn::Type> = None;
    let mut with_path: Option<syn::Path> = None;
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("as") {
            target_ty = Some(meta.value()?.parse()?);
            Ok(())
        } else if meta.path.is_ident("with") {
            with_path = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("unknown #[gpu(...)] argument -- expected `as`/`with`"))
        }
    })?;
    match (target_ty, with_path) {
        (Some(target_ty), Some(with_path)) => Ok(Some(GpuFieldOverride {
            target_ty,
            with_path,
        })),
        _ => Err(syn::Error::new_spanned(
            attr,
            "#[gpu(as = Type, with = path)] requires both `as` and `with` together",
        )),
    }
}

/// One `#[gpu]`-marked leaf field, ready to splice into the generated
/// mirror struct/conversion. Every field is wrapped in `pulsar_world_
/// registry::GpuRepr<FieldTy>` -- see that type's doc for why this is
/// universal (any `Copy` type, no per-shape classification) rather than a
/// hand-maintained list of "recognized" shapes: a `bool` stays 1 byte, an
/// enum stays whatever bytes its own `#[repr]` gives it, an already-Pod
/// custom struct passes through unchanged, all via the exact same wrapper.
/// A field whose type isn't `Copy` fails at the generated struct's own
/// `derive(Clone, Copy)` / `GpuRepr<T>` bound -- a plain, ordinary rustc
/// error at the point that's actually wrong, not a hand-rolled one here.
pub(crate) struct GpuLeafField {
    pub(crate) ident: syn::Ident,
    /// The field's own declared type (unwrapped -- `gpu_mirror_codegen`
    /// wraps it in `GpuRepr<..>` when splicing the mirror struct's field
    /// definition). Ignored in favor of `override_.target_ty` when an
    /// override is present.
    pub(crate) field_ty: syn::Type,
    /// `Some` for `#[gpu(as = ..., with = ...)]` -- see [`GpuFieldOverride`].
    pub(crate) override_: Option<GpuFieldOverride>,
}

/// Generates the composed `#[gpu]` mirror for one `#[engine_class]`-derived
/// struct (Pulsar-Native#561): a `Pod`, `#[derive(SceneStore)]`,
/// packed-layout companion struct (named `{Struct}GpuMirror`) holding one
/// field per `#[gpu]`-marked `#[property]` leaf PLUS one field per
/// `#[sub_props]` field (that sub-struct's own, independently-generated
/// `<SubTy as GpuMirrored>::GpuMirror` -- this is what makes nesting
/// compose with no special-casing: every `#[engine_class]`-derived struct,
/// `#[sub_props]` groups included, gets a `GpuMirrored` impl unconditionally,
/// so a containing struct never needs to know whether a given sub-props
/// group happens to contribute real fields this time or the zero-sized
/// `NoGpuMirror`).
///
/// The companion is never inserted as a component. The generated code
/// registers a SceneDB GPU dispatch for the authored struct itself that
/// derives the companion and writes its packed row
/// (`pulsar_scenedb::gpu::write_derived_row`), plus the matching clear, so the
/// row follows the authored value through SceneDB's own write lifecycle.
///
/// No error return: every `#[gpu]` field is accepted unconditionally (see
/// [`GpuLeafField`]'s doc) -- a type that genuinely can't work here (not
/// `Copy`) fails at the generated `GpuRepr<T>` field's own bound, an
/// ordinary rustc error pointing at the real cause.
pub(crate) fn gpu_mirror_codegen(
    name: &syn::Ident,
    gpu_leaf_fields: &[GpuLeafField],
    sub_props_fields: &[&Field],
    gpu_rows: bool,
) -> proc_macro2::TokenStream {
    if gpu_leaf_fields.is_empty() && sub_props_fields.is_empty() {
        // Nothing to mirror -- the trivial, common-case impl.
        return quote! {
            impl pulsar_world_registry::GpuMirrored for #name {
                type GpuMirror = pulsar_world_registry::NoGpuMirror;
                fn to_gpu_mirror(&self) -> pulsar_world_registry::NoGpuMirror {
                    pulsar_world_registry::NoGpuMirror
                }
            }
        };
    }

    let mirror_name = quote::format_ident!("{}GpuMirror", name);
    let dispatch_fn_name = quote::format_ident!("__pulsar_gpu_companion_dispatch_{}", name);
    let clear_fn_name = quote::format_ident!("__pulsar_gpu_companion_clear_{}", name);
    // Only an opted-in class (`#[engine_class(gpu_rows)]`) uploads its
    // companion as a SceneDB GPU row; otherwise the companion is a CPU
    // mapping helper (e.g. `LightComponent`'s, which its derived light row is
    // built from) and nothing is uploaded that no pass reads
    // (Pulsar-Native#1035, Phase 4).
    let gpu_row_registration = if gpu_rows {
        quote! {
        // The companion is not a component anyone inserts. `#name`'s own
        // SceneDB GPU dispatch derives it from the live value and writes its
        // packed row, so the row follows every insert (typed or erased),
        // `get_mut` write, removal, despawn and mirror replay of `#name`
        // through SceneDB's normal write path -- no refresh call, no second
        // stored copy to keep in step.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #dispatch_fn_name(
            mirror: &::pulsar_scenedb::gpu::GpuMirrorHandle,
            row: u32,
            data: *const (),
            is_new_insert: bool,
        ) {
            // SAFETY: SceneDB reaches this only through `#name`'s own
            // `ComponentId`, with a pointer to a live `#name`.
            let value = unsafe { &*(data as *const #name) };
            let derived = <#name as pulsar_world_registry::GpuMirrored>::to_gpu_mirror(value);
            ::pulsar_scenedb::gpu::write_derived_row(mirror, row, &derived, is_new_insert);
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #clear_fn_name(mirror: &::pulsar_scenedb::gpu::GpuMirrorHandle, row: u32) {
            ::pulsar_scenedb::gpu::clear_derived_row::<#mirror_name>(mirror, row);
        }

        ::pulsar_scenedb::pulsar_reflection::inventory::submit! {
            ::pulsar_scenedb::gpu::GpuMirrorRegistration {
                component_id: ::pulsar_scenedb::component_id::<#name>,
                dispatch: #dispatch_fn_name,
            }
        }

        ::pulsar_scenedb::pulsar_reflection::inventory::submit! {
            ::pulsar_scenedb::gpu::world_mirror::GpuClearRegistration {
                component_id: ::pulsar_scenedb::component_id::<#name>,
                clear: #clear_fn_name,
            }
        }
        }
    } else {
        quote! {}
    };

    let leaf_field_defs = gpu_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        let ty = leaf
            .override_
            .as_ref()
            .map(|o| &o.target_ty)
            .unwrap_or(&leaf.field_ty);
        quote! { #[gpu] pub #ident: pulsar_world_registry::GpuRepr<#ty> }
    });
    let leaf_field_inits = gpu_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        match &leaf.override_ {
            // `#[gpu(as = .., with = path)]` -- the transform runs exactly
            // here, once per `to_gpu_mirror()` call, not in some separate
            // hand-written pass a consumer has to remember to invoke.
            Some(o) => {
                let with_path = &o.with_path;
                quote! { #ident: pulsar_world_registry::GpuRepr(#with_path(self.#ident)) }
            }
            None => quote! { #ident: pulsar_world_registry::GpuRepr(self.#ident) },
        }
    });

    let sub_props_field_defs = sub_props_fields.iter().map(|field| {
        let ident = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        quote! { #[gpu] pub #ident: <#ty as pulsar_world_registry::GpuMirrored>::GpuMirror }
    });
    let sub_props_field_inits = sub_props_fields.iter().map(|field| {
        let ident = field.ident.as_ref().unwrap();
        quote! { #ident: pulsar_world_registry::GpuMirrored::to_gpu_mirror(&self.#ident) }
    });

    quote! {
        // `#[repr(C)]` is load-bearing, not cosmetic: this struct's own
        // field order matters not just for ITS packed buffer, but for
        // correctness when a CONTAINING struct's mirror embeds this one as
        // a plain field value (composition via #[sub_props]) -- the
        // containing struct's own packed view (`pulsar_scenedb_derive`'s
        // `#[repr(C)]`-guaranteed internal `__ScenedbGpuPacked_*` struct)
        // copies THIS struct's bytes as one opaque, `size_of`-sized blob,
        // which is only well-defined if this struct's OWN internal field
        // order is deterministic too -- without `#[repr(C)]` here, Rust's
        // default (unspecified) layout is free to reorder these fields,
        // silently corrupting every OUTER struct that nests this one.
        //
        // No `Default` in this derive list: every field is `GpuRepr<T>`,
        // which only derives `Default` if `T: Default` -- an unnecessary
        // constraint nothing here actually needs (see `pulsar_world_
        // registry::GpuMirrored`'s trait bound, which dropped it for the
        // same reason).
        #[derive(pulsar_scenedb::SceneStore, Clone, Copy)]
        #[gpu(layout = packed)]
        #[repr(C)]
        #[allow(non_snake_case)]
        pub struct #mirror_name {
            #(#leaf_field_defs,)*
            #(#sub_props_field_defs,)*
        }

        impl pulsar_world_registry::GpuMirrored for #name {
            type GpuMirror = #mirror_name;
            fn to_gpu_mirror(&self) -> #mirror_name {
                #mirror_name {
                    #(#leaf_field_inits,)*
                    #(#sub_props_field_inits,)*
                }
            }
        }

        #gpu_row_registration
    }
}
