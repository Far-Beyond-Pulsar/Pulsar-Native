//! Code generation shared by the attribute macros that register methods
//! with `pulsar_reflection::methods`: `#[reflect_methods]` in
//! `pulsar_reflection_derive`, and macros in other crates (e.g. SceneDB's
//! world-receiving component methods) whose shims take extra context
//! before the script-visible arguments.
//!
//! A [`MethodSpec`] is parsed from one method and produces the pieces of a
//! shim: argument validation and extraction, the call (with `Result`
//! unwrapping), and the `MethodInfo` literal describing the signature.
//! Every generated path is rooted at a caller-supplied path to the
//! `pulsar_reflection` crate, so a crate that only re-exports it works.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    spanned::Spanned, Attribute, Expr, ExprLit, FnArg, GenericArgument, Ident, ImplItemFn, Lit,
    Meta, Pat, PathArguments, ReturnType, Type,
};

/// How a method takes `self`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfKind {
    None,
    Ref,
    Mut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Value,
    Ref,
    Mut,
}

struct Param {
    name: String,
    /// The type stored in the argument slot. For `&str` this is `String`
    /// and for `&[T]` it is `Vec<T>`, since a slot must hold a sized value.
    slot_ty: Type,
    mode: Mode,
    /// `&str` / `&[T]`: pass the slot's contents as a slice.
    as_slice: bool,
}

/// The parsed signature and marker options of one registered method.
pub struct MethodSpec {
    pub ident: Ident,
    pub self_kind: SelfKind,
    /// The leading typed parameters that are context supplied by the shim
    /// (not script arguments), as written.
    pub context: Vec<Type>,
    script_name: String,
    doc: String,
    params: Vec<Param>,
    ret: Option<Type>,
    fallible: bool,
    side_effect_free: bool,
    deterministic: bool,
    attrs: Vec<(String, String)>,
}

impl MethodSpec {
    /// Parse `func`, whose marker attribute (already removed from `func`)
    /// is `marker`. The first `context` typed parameters are context, not
    /// script arguments.
    pub fn parse(func: &ImplItemFn, marker: &Attribute, context: usize) -> syn::Result<Self> {
        let sig = &func.sig;
        if !sig.generics.params.is_empty() {
            return Err(syn::Error::new_spanned(
                &sig.generics,
                "reflected methods cannot be generic",
            ));
        }
        if let Some(asyncness) = &sig.asyncness {
            return Err(syn::Error::new_spanned(
                asyncness,
                "reflected methods cannot be async",
            ));
        }

        let mut spec = MethodSpec {
            ident: sig.ident.clone(),
            self_kind: SelfKind::None,
            context: Vec::new(),
            script_name: sig.ident.to_string(),
            doc: doc_string(&func.attrs),
            params: Vec::new(),
            ret: None,
            fallible: false,
            side_effect_free: false,
            deterministic: false,
            attrs: Vec::new(),
        };
        spec.parse_marker(marker)?;

        for (index, input) in sig.inputs.iter().enumerate() {
            match input {
                FnArg::Receiver(receiver) => {
                    if receiver.colon_token.is_some() || receiver.reference.is_none() {
                        return Err(syn::Error::new_spanned(
                            receiver,
                            "reflected methods take `&self` or `&mut self`",
                        ));
                    }
                    spec.self_kind = if receiver.mutability.is_some() {
                        SelfKind::Mut
                    } else {
                        SelfKind::Ref
                    };
                }
                FnArg::Typed(typed) if spec.context.len() < context => {
                    spec.context.push((*typed.ty).clone());
                }
                FnArg::Typed(typed) => {
                    let name = match &*typed.pat {
                        Pat::Ident(ident) => ident.ident.to_string(),
                        _ => format!("arg{index}"),
                    };
                    spec.params.push(Param::parse(name, &typed.ty)?);
                }
            }
        }
        if spec.context.len() < context {
            return Err(syn::Error::new_spanned(
                &sig.inputs,
                format!("expected {context} leading context parameter(s)"),
            ));
        }

        if let ReturnType::Type(_, ty) = &sig.output {
            let (inner, fallible) = match result_ok_type(ty) {
                Some(ok) => (ok, true),
                None => ((**ty).clone(), false),
            };
            if matches!(inner, Type::Reference(_)) {
                return Err(syn::Error::new_spanned(
                    ty,
                    "reflected methods must return owned values",
                ));
            }
            spec.fallible = fallible;
            spec.ret = (!is_unit(&inner)).then_some(inner);
        }
        Ok(spec)
    }

    /// `#[marker]` or `#[marker(pure, name = "x", key = "v")]`.
    fn parse_marker(&mut self, marker: &Attribute) -> syn::Result<()> {
        if matches!(marker.meta, Meta::Path(_)) {
            return Ok(());
        }
        marker.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .ok_or_else(|| meta.error("expected a flag or `key = \"value\"`"))?;
            if meta.input.peek(syn::Token![=]) {
                let value: syn::LitStr = meta.value()?.parse()?;
                if key == "name" {
                    self.script_name = value.value();
                } else {
                    self.attrs.push((key, value.value()));
                }
                return Ok(());
            }
            match key.as_str() {
                "pure" => {
                    self.side_effect_free = true;
                    self.deterministic = true;
                }
                "side_effect_free" => self.side_effect_free = true,
                "deterministic" => self.deterministic = true,
                _ => {
                    return Err(meta.error(
                        "unknown flag; expected `pure`, `side_effect_free`, `deterministic` \
                         or `key = \"value\"`",
                    ))
                }
            }
            Ok(())
        })
    }

    /// Statements that check `args` (a `&mut [Box<dyn Any>]`) against the
    /// signature, returning `CallError` early, then bind the script
    /// arguments to `a0, a1, ..`. Nothing is moved out of `args` unless
    /// every check passes.
    pub fn extract_args(&self, reflection: &TokenStream, args: &Ident) -> TokenStream {
        let p = quote!(#reflection::methods::__private);
        let count = self.params.len();
        let checks = self.params.iter().enumerate().map(|(i, param)| {
            let ty = &param.slot_ty;
            quote!(#p::check_arg::<#ty>(#args, #i)?;)
        });
        let extracts = self
            .params
            .iter()
            .zip(self.arg_idents())
            .map(|(param, name)| {
                let ty = &param.slot_ty;
                let value = match param.mode {
                    Mode::Value => quote!(#p::take::<#ty>(slots.next().unwrap())),
                    Mode::Ref => quote!(#p::borrow::<#ty>(slots.next().unwrap())),
                    Mode::Mut => quote!(#p::borrow_mut::<#ty>(slots.next().unwrap())),
                };
                let value = match (param.as_slice, param.mode) {
                    (false, _) => value,
                    (true, Mode::Mut) => quote!(&mut #value[..]),
                    (true, _) => quote!(&#value[..]),
                };
                quote!(let #name = #value;)
            });
        quote! {
            #p::check_arg_count(#args, #count)?;
            #(#checks)*
            #[allow(unused_mut, unused_variables)]
            let mut slots = #args.iter_mut();
            #(#extracts)*
        }
    }

    /// `callee(leading.., a0, a1, ..)`, converted to the shim's
    /// `Result<Option<Box<dyn Any>>, CallError>`.
    pub fn call(
        &self,
        reflection: &TokenStream,
        callee: TokenStream,
        leading: &[TokenStream],
    ) -> TokenStream {
        let p = quote!(#reflection::methods::__private);
        let args = self.arg_idents();
        let call = quote!(#callee(#(#leading,)* #(#args),*));
        if self.fallible {
            quote!(#p::ret_result(#call))
        } else {
            quote!(::std::result::Result::Ok(#p::ret(#call)))
        }
    }

    /// The `MethodInfo { .. }` literal for this method (const-evaluable).
    pub fn info(&self, reflection: &TokenStream) -> TokenStream {
        let m = quote!(#reflection::methods);
        let name = &self.script_name;
        let doc = &self.doc;
        let params = self.params.iter().map(|param| {
            let name = &param.name;
            let ty = type_ref(reflection, &param.slot_ty);
            let mode = match param.mode {
                Mode::Value => quote!(#m::PassMode::Value),
                Mode::Ref => quote!(#m::PassMode::Ref),
                Mode::Mut => quote!(#m::PassMode::Mut),
            };
            quote!(#m::ParamInfo { name: #name, ty: #ty, mode: #mode })
        });
        let ret = match &self.ret {
            Some(ty) => {
                let ty = type_ref(reflection, ty);
                quote!(::std::option::Option::Some(#ty))
            }
            None => quote!(::std::option::Option::None),
        };
        let side_effect_free = self.side_effect_free;
        let deterministic = self.deterministic;
        let attrs = self.attrs.iter().map(|(k, v)| quote!((#k, #v)));
        quote! {
            #m::MethodInfo {
                name: #name,
                doc: #doc,
                params: &[#(#params),*],
                ret: #ret,
                flags: #m::MethodFlags {
                    side_effect_free: #side_effect_free,
                    deterministic: #deterministic,
                },
                attrs: &[#(#attrs),*],
            }
        }
    }

    fn arg_idents(&self) -> Vec<Ident> {
        (0..self.params.len())
            .map(|i| format_ident!("a{i}"))
            .collect()
    }
}

impl Param {
    fn parse(name: String, ty: &Type) -> syn::Result<Self> {
        let Type::Reference(reference) = ty else {
            if let Type::ImplTrait(_) = ty {
                return Err(syn::Error::new_spanned(
                    ty,
                    "reflected methods cannot take `impl Trait`",
                ));
            }
            return Ok(Self {
                name,
                slot_ty: ty.clone(),
                mode: Mode::Value,
                as_slice: false,
            });
        };
        let mode = if reference.mutability.is_some() {
            Mode::Mut
        } else {
            Mode::Ref
        };
        let (slot_ty, as_slice) = match &*reference.elem {
            Type::Path(path) if path.path.is_ident("str") => {
                if mode == Mode::Mut {
                    return Err(syn::Error::new_spanned(ty, "`&mut str` is not supported"));
                }
                (syn::parse_quote!(::std::string::String), true)
            }
            Type::Slice(slice) => {
                let inner = &slice.elem;
                (syn::parse_quote!(::std::vec::Vec<#inner>), true)
            }
            Type::Reference(_) => {
                return Err(syn::Error::new(
                    ty.span(),
                    "nested references are not supported",
                ));
            }
            other => (other.clone(), false),
        };
        Ok(Self {
            name,
            slot_ty,
            mode,
            as_slice,
        })
    }
}

/// `TypeRef { id, name }` literal for `ty`.
pub fn type_ref(reflection: &TokenStream, ty: &Type) -> TokenStream {
    quote! {
        #reflection::methods::TypeRef {
            id: ::std::any::TypeId::of::<#ty>,
            name: ::std::any::type_name::<#ty>,
        }
    }
}

/// Remove and return every method in `items` carrying the `marker`
/// attribute, paired with that attribute.
pub fn take_marked<'a>(
    items: &'a mut [syn::ImplItem],
    marker: &str,
) -> Vec<(&'a ImplItemFn, Attribute)> {
    items
        .iter_mut()
        .filter_map(|item| {
            let syn::ImplItem::Fn(func) = item else {
                return None;
            };
            let index = func.attrs.iter().position(|a| a.path().is_ident(marker))?;
            let attr = func.attrs.remove(index);
            Some((&*func, attr))
        })
        .collect()
}

/// The registration for an impl block's `#[reflect_method]` methods:
/// an `invoke` shim per method and a `ReflectedMethod` table (in a second
/// inherent impl of `self_ty`, so `Self` resolves), submitted to
/// `inventory` as a `TypeMethodRegistration`. `specs` must be non-empty
/// and parsed with no context parameters; `r` is the path to
/// `pulsar_reflection`.
pub fn reflected_registration(
    r: &TokenStream,
    self_ty: &Type,
    specs: &[MethodSpec],
) -> TokenStream {
    // Rust already rejects two inherent methods with the same name, so the
    // first method's name makes this block's const unique for the type.
    let const_name = format_ident!("__PULSAR_REFLECTED_METHODS_{}", specs[0].ident);
    let args = format_ident!("args");

    let shims = specs.iter().map(|spec| {
        let shim = format_ident!("__pulsar_reflect_invoke_{}", spec.ident);
        let ident = &spec.ident;
        let (receiver, leading) = match spec.self_kind {
            SelfKind::None => (quote!(let _ = receiver;), vec![]),
            SelfKind::Ref => (
                quote!(let this = receiver.downcast_ref::<Self>()?;),
                vec![quote!(this)],
            ),
            SelfKind::Mut => (
                quote!(let mut receiver = receiver; let this = receiver.downcast_mut::<Self>()?;),
                vec![quote!(this)],
            ),
        };
        let extract = spec.extract_args(r, &args);
        let call = spec.call(r, quote!(Self::#ident), &leading);
        quote! {
            #[doc(hidden)]
            fn #shim(
                receiver: #r::methods::Receiver<'_>,
                #args: &mut [::std::boxed::Box<dyn ::std::any::Any>],
            ) -> ::std::result::Result<
                ::std::option::Option<::std::boxed::Box<dyn ::std::any::Any>>,
                #r::methods::CallError,
            > {
                #receiver
                #extract
                #call
            }
        }
    });
    let entries = specs.iter().map(|spec| {
        let shim = format_ident!("__pulsar_reflect_invoke_{}", spec.ident);
        let info = spec.info(r);
        let receiver = match spec.self_kind {
            SelfKind::None => quote!(#r::methods::ReceiverKind::None),
            SelfKind::Ref => quote!(#r::methods::ReceiverKind::Ref),
            SelfKind::Mut => quote!(#r::methods::ReceiverKind::Mut),
        };
        quote!(#r::methods::ReflectedMethod { info: #info, receiver: #receiver, invoke: Self::#shim })
    });
    let ty = type_ref(r, self_ty);

    quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, non_upper_case_globals, clippy::needless_borrow, clippy::unit_arg)]
        impl #self_ty {
            #(#shims)*

            #[doc(hidden)]
            const #const_name: &'static [#r::methods::ReflectedMethod] = &[#(#entries),*];
        }

        #r::inventory::submit! {
            #r::methods::TypeMethodRegistration { ty: #ty, methods: <#self_ty>::#const_name }
        }
    }
}

fn is_unit(ty: &Type) -> bool {
    matches!(ty, Type::Tuple(tuple) if tuple.elems.is_empty())
}

/// `T` for a return type spelled `Result<T, ..>` (any path ending in
/// `Result`, so aliases like `anyhow::Result<T>` count too).
fn result_ok_type(ty: &Type) -> Option<Type> {
    let Type::Path(path) = ty else { return None };
    let last = path.path.segments.last()?;
    if last.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(ok) => Some(ok.clone()),
        _ => None,
    }
}

fn doc_string(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').unwrap_or(&line).to_string())
        .collect();
    lines.join("\n").trim().to_string()
}
