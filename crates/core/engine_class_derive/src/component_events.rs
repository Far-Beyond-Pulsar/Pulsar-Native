//! `#[component_events]`: turns a component's event methods into registered
//! Blueprint events. Entry point: [`expand`].

use std::collections::HashSet;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use syn::{ItemTrait, parse::Parser};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand_events(attr, item) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// What the macro generates for the whole trait, one event method at a time.
#[derive(Default)]
struct EventOutput {
    /// Declarations in the generated emitter trait.
    event_methods: Vec<TokenStream2>,
    /// Their implementations on the component event writer.
    event_impl_methods: Vec<TokenStream2>,
    /// The descriptor and declaration functions plus their registration.
    descriptor_submits: Vec<TokenStream2>,
    /// Encode/decode functions and script type registrations for payloads.
    payload_registrations: Vec<TokenStream2>,
    /// Payload types already registered, keyed by their token text.
    registered_payloads: HashSet<String>,
}

/// What every event method in the trait shares.
struct EventTrait<'a> {
    class_name: &'a syn::LitStr,
    trait_name: &'a syn::Ident,
}

/// The pieces of an event method's arguments.
struct EventArguments {
    parameters: Vec<TokenStream2>,
    values: Vec<TokenStream2>,
    declaration_fields: Vec<TokenStream2>,
    descriptor_fields: Vec<TokenStream2>,
}

fn expand_events(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream2> {
    let class_name = parse_class_name(attr)?;
    let mut event_trait = syn::parse::<ItemTrait>(item)?;
    if !event_trait.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &event_trait.generics,
            "component event traits cannot be generic",
        ));
    }
    let trait_name = event_trait.ident.clone();
    let visibility = event_trait.vis.clone();
    let emitter_name = quote::format_ident!("{}EventWriterExt", trait_name);
    let original_items = std::mem::take(&mut event_trait.items);
    let shared = EventTrait {
        class_name: &class_name,
        trait_name: &trait_name,
    };
    let mut output = EventOutput::default();
    for trait_item in original_items {
        expand_event_method(trait_item, &shared, &mut output)?;
    }
    let EventOutput {
        event_methods,
        event_impl_methods,
        descriptor_submits,
        payload_registrations,
        ..
    } = output;
    Ok(quote::quote! {
        #event_trait

        #visibility trait #emitter_name {
            #(#event_methods)*
        }

        impl<'__pulsar_event> #emitter_name for pulsar_world_registry::ComponentEventWriter<'__pulsar_event> {
            #(#event_impl_methods)*
        }

        #(#payload_registrations)*
        #(#descriptor_submits)*
    })
}

/// Reads the `class = "ComponentClassName"` argument.
fn parse_class_name(attr: TokenStream) -> syn::Result<syn::LitStr> {
    let metas =
        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated.parse(attr)?;
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
    class_name.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "expected `class = \"ComponentClassName\"`",
        )
    })
}

/// Validates one trait item as an event method and generates its emitter
/// declaration, implementation and registration.
fn expand_event_method(
    trait_item: syn::TraitItem,
    shared: &EventTrait,
    out: &mut EventOutput,
) -> syn::Result<()> {
    let method = take_event_method(trait_item)?;
    let payload_type = event_payload_type(&method)?;
    let class_name = shared.class_name;
    let trait_name = shared.trait_name;
    let event_name = format!("{}.{}", class_name.value(), method.sig.ident);
    let method_name = method.sig.ident.clone();
    let descriptor_fn =
        quote::format_ident!("__pulsar_event_descriptor_{}_{}", trait_name, method_name);
    let declaration_fn =
        quote::format_ident!("__pulsar_event_declaration_{}_{}", trait_name, method_name);

    let arguments = encode_event_arguments(&method, trait_name, out)?;
    let (payload_name, _payload_base) = register_event_value(
        payload_type,
        trait_name,
        &mut out.payload_registrations,
        &mut out.registered_payloads,
    )?;
    let payload_parameter = quote::format_ident!("__pulsar_payload");
    let payload_value_type = payload_type;
    let EventArguments {
        parameters: argument_parameters,
        values: argument_values,
        declaration_fields,
        descriptor_fields,
    } = arguments;
    let mut all_event_fields = declaration_fields;
    all_event_fields.push(quote::quote!(pulsar_world_registry::pulsar_script_vm::EventField::new("payload", pulsar_world_registry::pulsar_script_vm::Type::Object(#payload_name.to_owned()))));
    let mut all_descriptor_fields = descriptor_fields;
    all_descriptor_fields.push(quote::quote!((
        "payload",
        pulsar_events::gamma::FieldType::Bytes
    )));
    out.event_methods.push(quote::quote! {
        fn #method_name(&self, #(#argument_parameters,)* #payload_parameter: #payload_value_type) -> Result<(), String>
        where
            #payload_value_type: Clone + Send + Sync + 'static,
        ;
    });
    out.event_impl_methods.push(quote::quote! {
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
    out.descriptor_submits.push(quote::quote! {
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
    Ok(())
}

/// Checks that a trait item is a non-generic `#[bp_event]` method and strips
/// the `bp_event` marker from it.
fn take_event_method(trait_item: syn::TraitItem) -> syn::Result<syn::TraitItemFn> {
    let syn::TraitItem::Fn(mut method) = trait_item else {
        return Err(syn::Error::new_spanned(
            trait_item,
            "component event traits may contain only event methods",
        ));
    };
    let is_bp_event = method
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("bp_event"));
    if !is_bp_event {
        return Err(syn::Error::new_spanned(
            &method,
            "event methods must be marked `#[bp_event]`",
        ));
    }
    method
        .attrs
        .retain(|attr| !attr.path().is_ident("bp_event"));
    if !method.sig.generics.params.is_empty() || method.sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "component event declarations cannot be generic or variadic",
        ));
    }
    Ok(method)
}

/// The payload type an event method returns, which must be a concrete named
/// value type.
fn event_payload_type(method: &syn::TraitItemFn) -> syn::Result<&syn::Type> {
    let syn::ReturnType::Type(_, payload_type) = &method.sig.output else {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "event declarations must return their payload type",
        ));
    };
    if !matches!(payload_type.as_ref(), syn::Type::Path(path) if path.qself.is_none() && path.path.segments.last().is_some_and(|segment| matches!(segment.arguments, syn::PathArguments::None)))
    {
        return Err(syn::Error::new_spanned(
            payload_type,
            "event return payloads must be owned, concrete named Rust value types",
        ));
    }
    Ok(payload_type.as_ref())
}

/// Registers each argument's value type and builds the parameter, encoding,
/// declaration and descriptor tokens for it.
fn encode_event_arguments(
    method: &syn::TraitItemFn,
    trait_name: &syn::Ident,
    out: &mut EventOutput,
) -> syn::Result<EventArguments> {
    let mut arguments = EventArguments {
        parameters: Vec::new(),
        values: Vec::new(),
        declaration_fields: Vec::new(),
        descriptor_fields: Vec::new(),
    };
    for input in &method.sig.inputs {
        let syn::FnArg::Typed(input) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "event methods cannot have a receiver",
            ));
        };
        let syn::Pat::Ident(pattern) = input.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                &input.pat,
                "event arguments must use named identifiers",
            ));
        };
        let name = &pattern.ident;
        let name_str = name.to_string();
        let argument_type = &input.ty;
        let (value_name, _base_type) = register_event_value(
            &input.ty,
            trait_name,
            &mut out.payload_registrations,
            &mut out.registered_payloads,
        )?;
        let (_, by_ref) = event_value_type(&input.ty).expect(
            "register_event_value above already rejected a type event_value_type cannot read",
        );
        let owned_value = if by_ref {
            quote::quote!((*#name).clone())
        } else {
            quote::quote!(#name.clone())
        };
        arguments
            .parameters
            .push(quote::quote!(#name: #argument_type));
        arguments.values.push(quote::quote! {
            fields.push(pulsar_events::gamma::DynValue::Bytes(
                pulsar_world_registry::pulsar_script_vm::TypeRegistry::global()
                    .encode_event_value(&pulsar_world_registry::pulsar_script_vm::Object::new(#value_name, #owned_value))?
            ));
        });
        arguments.declaration_fields.push(quote::quote!(pulsar_world_registry::pulsar_script_vm::EventField::new(#name_str, pulsar_world_registry::pulsar_script_vm::Type::Object(#value_name.to_owned()))));
        arguments
            .descriptor_fields
            .push(quote::quote!((#name_str, pulsar_events::gamma::FieldType::Bytes)));
    }
    Ok(arguments)
}

/// The base type of an event value and whether it was a shared reference.
fn event_value_type(ty: &syn::Type) -> Option<(&syn::Type, bool)> {
    match ty {
        syn::Type::Path(path) if path.qself.is_none() => Some((ty, false)),
        syn::Type::Reference(reference) if reference.mutability.is_none() => {
            match reference.elem.as_ref() {
                syn::Type::Path(path) if path.qself.is_none() => Some((&reference.elem, true)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Registers the codec and script type for an event value type (once per
/// type) and returns its script name and base type.
fn register_event_value(
    ty: &syn::Type,
    trait_name: &syn::Ident,
    registrations: &mut Vec<TokenStream2>,
    registered: &mut HashSet<String>,
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
        let encode_fn = quote::format_ident!("__pulsar_event_encode_{}_{}", trait_name, ident);
        let decode_fn = quote::format_ident!("__pulsar_event_decode_{}_{}", trait_name, ident);
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
