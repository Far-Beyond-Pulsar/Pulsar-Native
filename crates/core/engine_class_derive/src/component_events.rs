//! `#[component_events]`: turns a component's event methods into registered
//! Blueprint events. Entry point: [`expand`].

use proc_macro::TokenStream;
use syn::{ItemTrait, parse::Parser, parse_macro_input};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let metas = match syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
        .parse(attr)
    {
        Ok(metas) => metas,
        Err(error) => return error.to_compile_error().into(),
    };
    let class_name = metas.iter().find_map(|meta| match meta {
        syn::Meta::NameValue(value) if value.path.is_ident("class") => match &value.value {
            syn::Expr::Lit(expr) => match &expr.lit {
                syn::Lit::Str(value) => Some(value.clone()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    });
    let Some(class_name) = class_name else {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "expected `class = \"ComponentClassName\"`",
        )
        .to_compile_error()
        .into();
    };
    let mut event_trait = parse_macro_input!(item as ItemTrait);
    if !event_trait.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &event_trait.generics,
            "component event traits cannot be generic",
        )
        .to_compile_error()
        .into();
    }
    let trait_name = event_trait.ident.clone();
    let visibility = event_trait.vis.clone();
    let emitter_name = quote::format_ident!("{}EventWriterExt", trait_name);
    let original_items = std::mem::take(&mut event_trait.items);
    let mut event_methods = Vec::new();
    let mut event_impl_methods = Vec::new();
    let mut descriptor_submits = Vec::new();
    let mut payload_registrations = Vec::new();
    let mut registered_payloads = std::collections::HashSet::new();
    for trait_item in original_items {
        let syn::TraitItem::Fn(mut method) = trait_item else {
            return syn::Error::new_spanned(
                trait_item,
                "component event traits may contain only event methods",
            )
            .to_compile_error()
            .into();
        };
        let is_bp_event = method
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("bp_event"));
        if !is_bp_event {
            return syn::Error::new_spanned(&method, "event methods must be marked `#[bp_event]`")
                .to_compile_error()
                .into();
        }
        method
            .attrs
            .retain(|attr| !attr.path().is_ident("bp_event"));
        if !method.sig.generics.params.is_empty() || method.sig.variadic.is_some() {
            return syn::Error::new_spanned(
                &method.sig,
                "component event declarations cannot be generic or variadic",
            )
            .to_compile_error()
            .into();
        }
        let syn::ReturnType::Type(_, payload_type) = &method.sig.output else {
            return syn::Error::new_spanned(
                &method.sig,
                "event declarations must return their payload type",
            )
            .to_compile_error()
            .into();
        };
        if !matches!(payload_type.as_ref(), syn::Type::Path(path) if path.qself.is_none() && path.path.segments.last().is_some_and(|segment| matches!(segment.arguments, syn::PathArguments::None)))
        {
            return syn::Error::new_spanned(
                payload_type,
                "event return payloads must be owned, concrete named Rust value types",
            )
            .to_compile_error()
            .into();
        }
        let event_name = format!("{}.{}", class_name.value(), method.sig.ident);
        let method_name = method.sig.ident.clone();
        let descriptor_fn =
            quote::format_ident!("__pulsar_event_descriptor_{}_{}", trait_name, method_name);
        let declaration_fn =
            quote::format_ident!("__pulsar_event_declaration_{}_{}", trait_name, method_name);
        fn event_value_type(ty: &syn::Type) -> Option<(&syn::Type, bool)> {
            match ty {
                syn::Type::Path(path) if path.qself.is_none() => Some((ty, false)),
                syn::Type::Reference(reference) if reference.mutability.is_none() => {
                    match reference.elem.as_ref() {
                        syn::Type::Path(path) if path.qself.is_none() => {
                            Some((&reference.elem, true))
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        fn register_event_value(
            ty: &syn::Type,
            trait_name: &syn::Ident,
            registrations: &mut Vec<proc_macro2::TokenStream>,
            registered: &mut std::collections::HashSet<String>,
        ) -> Result<(String, syn::Type), syn::Error> {
            let Some((base, _)) = event_value_type(ty) else {
                return Err(syn::Error::new_spanned(
                    ty,
                    "event fields must be named, non-generic Rust value types (or shared references to them)",
                ));
            };
            let syn::Type::Path(path) = base else {
                unreachable!()
            };
            let ident = path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.clone())
                .expect("nonempty type path");
            if path
                .path
                .segments
                .last()
                .is_some_and(|segment| !matches!(segment.arguments, syn::PathArguments::None))
            {
                return Err(syn::Error::new_spanned(
                    base,
                    "generic event value types need a concrete registered wrapper type",
                ));
            }
            let name = ident.to_string();
            let key = quote::quote!(#base).to_string();
            if registered.insert(key) {
                let encode_fn =
                    quote::format_ident!("__pulsar_event_encode_{}_{}", trait_name, ident);
                let decode_fn =
                    quote::format_ident!("__pulsar_event_decode_{}_{}", trait_name, ident);
                registrations.push(quote::quote! {
                    #[doc(hidden)]
                    #[allow(non_snake_case)]
                    fn #encode_fn(value: &#base) -> Result<Vec<u8>, String> {
                        ::serde_json::to_vec(value).map_err(|error| error.to_string())
                    }
                    #[doc(hidden)]
                    #[allow(non_snake_case)]
                    fn #decode_fn(bytes: &[u8]) -> Result<#base, String> {
                        ::serde_json::from_slice(bytes).map_err(|error| error.to_string())
                    }
                    pulsar_world_registry::pulsar_script_vm::script_value_type!(#base, #name);
                    pulsar_world_registry::pulsar_script_vm::script_event_codec!(
                        #base, #name, encode = #encode_fn, decode = #decode_fn,
                    );
                });
            }
            Ok((name, base.clone()))
        }
        let mut argument_parameters = Vec::new();
        let mut argument_values = Vec::new();
        let mut declaration_fields = Vec::new();
        let mut descriptor_fields = Vec::new();
        for input in &method.sig.inputs {
            let syn::FnArg::Typed(input) = input else {
                return syn::Error::new_spanned(input, "event methods cannot have a receiver")
                    .to_compile_error()
                    .into();
            };
            let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
                return syn::Error::new_spanned(
                    &input.pat,
                    "event arguments must use named identifiers",
                )
                .to_compile_error()
                .into();
            };
            let name = &pattern.ident;
            let name_str = name.to_string();
            let argument_type = &input.ty;
            let (value_name, base_type) = match register_event_value(
                &input.ty,
                &trait_name,
                &mut payload_registrations,
                &mut registered_payloads,
            ) {
                Ok(value) => value,
                Err(error) => return error.to_compile_error().into(),
            };
            let (_, by_ref) = event_value_type(&input.ty).unwrap();
            let _ = base_type;
            let owned_value = if by_ref {
                quote::quote!((*#name).clone())
            } else {
                quote::quote!(#name.clone())
            };
            argument_parameters.push(quote::quote!(#name: #argument_type));
            argument_values.push(quote::quote! {
                fields.push(pulsar_events::gamma::DynValue::Bytes(
                    pulsar_world_registry::pulsar_script_vm::TypeRegistry::global()
                        .encode_event_value(&pulsar_world_registry::pulsar_script_vm::Object::new(#value_name, #owned_value))?
                ));
            });
            declaration_fields.push(quote::quote!(pulsar_world_registry::pulsar_script_vm::EventField::new(#name_str, pulsar_world_registry::pulsar_script_vm::Type::Object(#value_name.to_owned()))));
            descriptor_fields
                .push(quote::quote!((#name_str, pulsar_events::gamma::FieldType::Bytes)));
        }
        let (payload_name, payload_base) = match register_event_value(
            payload_type,
            &trait_name,
            &mut payload_registrations,
            &mut registered_payloads,
        ) {
            Ok(value) => value,
            Err(error) => return error.to_compile_error().into(),
        };
        let _ = payload_base;
        let payload_parameter = quote::format_ident!("__pulsar_payload");
        let payload_value_type = payload_type;
        let mut all_event_fields = declaration_fields;
        all_event_fields.push(quote::quote!(pulsar_world_registry::pulsar_script_vm::EventField::new("payload", pulsar_world_registry::pulsar_script_vm::Type::Object(#payload_name.to_owned()))));
        let mut all_descriptor_fields = descriptor_fields;
        all_descriptor_fields.push(quote::quote!((
            "payload",
            pulsar_events::gamma::FieldType::Bytes
        )));
        event_methods.push(quote::quote! {
            fn #method_name(&self, #(#argument_parameters,)* #payload_parameter: #payload_value_type) -> Result<(), String>
            where
                #payload_value_type: Clone + Send + Sync + 'static,
            ;
        });
        event_impl_methods.push(quote::quote! {
            fn #method_name(&self, #(#argument_parameters,)* #payload_parameter: #payload_value_type) -> Result<(), String>
            where
                #payload_value_type: Clone + Send + Sync + 'static,
            {
                let mut fields = Vec::new();
                #(#argument_values)*
                fields.push(pulsar_events::gamma::DynValue::Bytes(
                    pulsar_world_registry::pulsar_script_vm::TypeRegistry::global()
                        .encode_event_value(&pulsar_world_registry::pulsar_script_vm::Object::new(#payload_name, #payload_parameter.clone()))?
                ));
                self.emit_named(#event_name, fields)
            }
        });
        descriptor_submits.push(quote::quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #descriptor_fn() -> pulsar_events::gamma::EventDescriptor {
                pulsar_events::gamma::EventDescriptor::dynamic(
                    #event_name,
                    [#(#all_descriptor_fields),*],
                )
            }
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #declaration_fn() -> pulsar_world_registry::pulsar_script_vm::EventDecl {
                pulsar_world_registry::pulsar_script_vm::EventDecl {
                    name: #event_name.to_owned(),
                    fields: vec![#(#all_event_fields),*],
                }
            }
            pulsar_world_registry::inventory::submit! {
                pulsar_world_registry::ComponentEventRegistration {
                    class_name: #class_name,
                    descriptor: #descriptor_fn,
                    declaration: #declaration_fn,
                }
            }
        });
    }
    quote::quote! {
        #event_trait

        #visibility trait #emitter_name {
            #(#event_methods)*
        }

        impl<'__pulsar_event> #emitter_name for pulsar_world_registry::ComponentEventWriter<'__pulsar_event> {
            #(#event_impl_methods)*
        }

        #(#payload_registrations)*
        #(#descriptor_submits)*
    }
    .into()
}
