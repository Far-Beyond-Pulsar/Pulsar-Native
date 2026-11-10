//! `#[register_component_runtime(...)]`: registers a component's runtime
//! behavior (its `tick`, optional `begin_play` / `end_play` and
//! `#[bp_handler]` event handlers) with `pulsar_world_registry`.
//!
//! Entry point: [`expand`], a short orchestrator over the named steps below:
//! [`parse_args`], [`collect_handlers`], [`validate_lifecycle`] and
//! [`generate`]. Every step reports problems as a `syn::Error`, which
//! [`expand`] turns into a compile error at the offending span.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ItemImpl, parse::Parser, parse_macro_input};

/// The attribute's optional `enabled = field` and `class = "Name"` options.
struct RuntimeArgs {
    enabled_field: Option<syn::Ident>,
    explicit_class: Option<String>,
}

/// Native `#[bp_handler]` methods found in the impl: their dispatch arms and
/// the event names they handle.
struct Handlers {
    calls: Vec<TokenStream2>,
    events: std::collections::BTreeSet<String>,
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_args(attr) {
        Ok(args) => args,
        Err(error) => return error.to_compile_error().into(),
    };
    let mut impl_block = parse_macro_input!(item as ItemImpl);
    let result = (|| -> syn::Result<TokenStream2> {
        let ty_ident = component_type_ident(&impl_block)?;
        let class_name = args
            .explicit_class
            .clone()
            .unwrap_or_else(|| ty_ident.to_string());
        let handlers = collect_handlers(&mut impl_block, &class_name)?;
        validate_lifecycle(&impl_block)?;
        Ok(generate(
            &impl_block,
            &ty_ident,
            &class_name,
            &args,
            &handlers,
        ))
    })();
    match result {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Parse the attribute options, rejecting anything but `enabled` / `class`
/// so a typo fails at the attribute.
fn parse_args(attr: TokenStream) -> syn::Result<RuntimeArgs> {
    let args = match syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
        .parse(attr)
    {
        Ok(args) => args,
        Err(error) => return Err(error),
    };
    let enabled_field = args.iter().find_map(|meta| match meta {
        syn::Meta::NameValue(value) if value.path.is_ident("enabled") => match &value.value {
            syn::Expr::Path(path) => path.path.get_ident().cloned(),
            _ => None,
        },
        _ => None,
    });
    let explicit_class = args.iter().find_map(|meta| match meta {
        syn::Meta::NameValue(value) if value.path.is_ident("class") => match &value.value {
            syn::Expr::Lit(expr) => match &expr.lit {
                syn::Lit::Str(value) => Some(value.value()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    });
    if args.iter().any(|meta| !matches!(meta, syn::Meta::NameValue(value) if value.path.is_ident("enabled") || value.path.is_ident("class")))
        || (args.iter().any(|meta| matches!(meta, syn::Meta::NameValue(value) if value.path.is_ident("enabled"))) && enabled_field.is_none())
        || (args.iter().any(|meta| matches!(meta, syn::Meta::NameValue(value) if value.path.is_ident("class"))) && explicit_class.is_none())
    {
        return Err(syn::Error::new(proc_macro2::Span::call_site(), "expected optional `enabled = field_name` and/or `class = \"ComponentClass\"`"));
    }

    Ok(RuntimeArgs {
        enabled_field,
        explicit_class,
    })
}

/// The impl's self type must be a plain named type on a non-generic
/// inherent impl; returns its last path segment.
fn component_type_ident(impl_block: &ItemImpl) -> syn::Result<syn::Ident> {
    if impl_block.trait_.is_some() || !impl_block.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &impl_block,
            "#[register_component_runtime] requires a non-generic inherent impl block",
        ));
    }
    let self_ty = &impl_block.self_ty;
    let Some(ty_ident) = (match &**self_ty {
        syn::Type::Path(path) => path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }) else {
        return Err(syn::Error::new_spanned(
            self_ty,
            "expected a named component type",
        ));
    };
    Ok(ty_ident.clone())
}

/// Find every `#[bp_handler("event")]` method, validate its signature and
/// build its dispatch arm (decode each payload field, then call the method).
/// The attribute is removed from the emitted impl.
fn collect_handlers(impl_block: &mut ItemImpl, class_name: &str) -> syn::Result<Handlers> {
    let mut handler_calls = Vec::new();
    let mut handler_events = std::collections::BTreeSet::new();
    for item in &mut impl_block.items {
        let syn::ImplItem::Fn(method) = item else {
            continue;
        };
        let Some(handler_attr_index) = method
            .attrs
            .iter()
            .position(|attr| attr.path().is_ident("bp_handler"))
        else {
            continue;
        };
        let handler_attr = method.attrs.remove(handler_attr_index);
        let event_short_name = handler_attr.parse_args::<syn::LitStr>();
        let event_short_name = match event_short_name {
            Ok(name) => name.value(),
            Err(error) => {
                return Err(syn::Error::new_spanned(
                    handler_attr,
                    format!("expected #[bp_handler(\"event_name\")]: {error}"),
                ));
            }
        };
        if method.sig.inputs.len() < 3
            || !matches!(method.sig.inputs.first(), Some(syn::FnArg::Receiver(receiver)) if receiver.reference.is_some() && receiver.mutability.is_some())
            || !matches!(method.sig.inputs.iter().nth(1), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, syn::Type::Reference(reference) if matches!(&*reference.elem, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "ComponentContext"))))
            || !matches!(method.sig.output, syn::ReturnType::Default)
        {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "bp_handler must have signature `fn handler(&mut self, context: &mut ComponentContext, event_argument..., payload)`",
            ));
        }
        let event_name = format!("{class_name}.{event_short_name}");
        let method_name = method.sig.ident.clone();
        if !handler_events.insert(event_name.clone()) {
            return Err(syn::Error::new_spanned(
                &method.sig,
                format!("multiple native handlers target component event `{event_name}`"),
            ));
        }
        let mut decoded_names = Vec::new();
        let mut decode_fields = Vec::new();
        for (index, arg) in method.sig.inputs.iter().skip(2).enumerate() {
            let syn::FnArg::Typed(arg) = arg else {
                unreachable!("validated")
            };
            let syn::Pat::Ident(pattern) = arg.pat.as_ref() else {
                return Err(syn::Error::new_spanned(
                    &arg.pat,
                    "native event handler arguments must use named identifiers",
                ));
            };
            let syn::Type::Path(path) = arg.ty.as_ref() else {
                return Err(syn::Error::new_spanned(
                    &arg.ty,
                    "native event handler arguments must be owned named value types",
                ));
            };
            if path.qself.is_some()
                || path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| !matches!(segment.arguments, syn::PathArguments::None))
            {
                return Err(syn::Error::new_spanned(
                    &arg.ty,
                    "native event handler arguments must use concrete registered value types",
                ));
            }
            let payload_ident = path.path.segments.last().unwrap().ident.clone();
            let payload_name = payload_ident.to_string();
            let local = quote::format_ident!("__pulsar_handler_arg_{index}");
            let payload_type = &arg.ty;
            decoded_names.push(local.clone());
            decode_fields.push(quote::quote! {
                let Some(pulsar_events::gamma::DynValue::Bytes(bytes)) = queued_event.fields.get(#index) else {
                    tracing::warn!(event = #event_name, "native component event handler received malformed payload fields");
                    continue;
                };
                let decoded = pulsar_world_registry::pulsar_script_vm::TypeRegistry::global()
                    .decode_event_value(#payload_name, bytes);
                let #local = match decoded.and_then(|object| {
                    object.downcast_ref::<#payload_type>()
                        .cloned()
                        .ok_or_else(|| format!("decoded event field was not {}", #payload_name))
                }) {
                    Ok(value) => value,
                    Err(error) => {
                        tracing::warn!(event = #event_name, %error, "native component event handler field decode failed");
                        continue;
                    }
                };
            });
            let _ = pattern;
        }
        let handler_field_count = decoded_names.len();
        handler_calls.push(quote::quote! {
            #event_name => {
                if queued_event.fields.len() != #handler_field_count {
                    tracing::warn!(event = #event_name, expected = #handler_field_count, actual = queued_event.fields.len(), "native component event handler received the wrong field count");
                    continue;
                }
                #(#decode_fields)*
                component.#method_name(&mut context, #(#decoded_names),*);
            }
        });
    }
    Ok(Handlers {
        calls: handler_calls,
        events: handler_events,
    })
}

/// Check the `tick`, `begin_play` and `end_play` signatures the generated
/// shims call, so a mismatch is an error on the method, not in generated code.
fn validate_lifecycle(impl_block: &ItemImpl) -> syn::Result<()> {
    let tick = find_method(impl_block, "tick");
    let Some(tick) = tick else {
        return Err(syn::Error::new_spanned(
            &impl_block,
            "the impl must define `fn tick(&mut self, context: &mut ComponentContext, delta_seconds: f32)`",
        ));
    };
    let tick_sig = &tick.sig;
    let valid_args = tick_sig.inputs.len() == 3
        && matches!(tick_sig.inputs.first(), Some(syn::FnArg::Receiver(receiver)) if receiver.reference.is_some() && receiver.mutability.is_some())
        && matches!(tick_sig.inputs.iter().nth(1), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, syn::Type::Reference(reference) if matches!(&*reference.elem, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "ComponentContext"))))
        && matches!(tick_sig.inputs.iter().nth(2), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, syn::Type::Path(path) if path.path.is_ident("f32")));
    if !valid_args || !matches!(tick_sig.output, syn::ReturnType::Default) {
        return Err(syn::Error::new_spanned(
            tick_sig,
            "tick must have signature `fn tick(&mut self, context: &mut ComponentContext, delta_seconds: f32)` and return `()`",
        ));
    }

    let begin_play = find_method(impl_block, "begin_play");
    if let Some(begin_play) = begin_play {
        let sig = &begin_play.sig;
        let valid = sig.inputs.len() == 2
            && matches!(sig.inputs.first(), Some(syn::FnArg::Receiver(receiver)) if receiver.reference.is_some() && receiver.mutability.is_some())
            && matches!(sig.inputs.iter().nth(1), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, syn::Type::Reference(reference) if matches!(&*reference.elem, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "ComponentContext"))))
            && matches!(sig.output, syn::ReturnType::Default);
        if !valid {
            return Err(syn::Error::new_spanned(
                sig,
                "begin_play must have signature `fn begin_play(&mut self, context: &mut ComponentContext)`",
            ));
        }
    }
    let end_play = find_method(impl_block, "end_play");
    if let Some(end_play) = end_play {
        let sig = &end_play.sig;
        let valid = sig.inputs.len() == 2
            && matches!(sig.inputs.first(), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Entity")))
            && matches!(sig.inputs.iter().nth(1), Some(syn::FnArg::Typed(arg)) if matches!(&*arg.ty, syn::Type::Reference(reference) if matches!(&*reference.elem, syn::Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "ComponentContext"))))
            && matches!(sig.output, syn::ReturnType::Default);
        if !valid {
            return Err(syn::Error::new_spanned(
                sig,
                "end_play must have signature `fn end_play(entity: Entity, context: &mut ComponentContext)`",
            ));
        }
    }
    Ok(())
}

/// Emit the impl unchanged plus the tick shim (subscribe, dispatch queued
/// events, `begin_play` on the first frame, `tick`) and its registration.
fn generate(
    impl_block: &ItemImpl,
    ty_ident: &syn::Ident,
    class_name: &str,
    args: &RuntimeArgs,
    handlers: &Handlers,
) -> TokenStream2 {
    let self_ty = &impl_block.self_ty;
    let handler_calls = &handlers.calls;
    let handler_events = &handlers.events;
    let enabled_field = args.enabled_field.clone();
    let tick = find_method(impl_block, "tick");
    let begin_play = find_method(impl_block, "begin_play");
    let end_play = find_method(impl_block, "end_play");
    let shim = quote::format_ident!("__pulsar_component_tick_{}", ty_ident);
    let end_shim = quote::format_ident!("__pulsar_component_end_{}", ty_ident);
    let enabled = enabled_field.map(|field| quote::quote! { component.#field });
    let begin_call = begin_play.map(|_| quote::quote! { component.begin_play(&mut context); });
    let end_callback = end_play.map(|_| {
        quote::quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #end_shim(entity: pulsar_scenedb::Entity, events: &pulsar_events::EventHub) {
                let mut context = pulsar_world_registry::ComponentContext::new(entity, events);
                #self_ty::end_play(entity, &mut context);
            }
        }
    });
    let end_callback_value = end_play
        .map(|_| quote::quote! { Some(#end_shim) })
        .unwrap_or_else(|| quote::quote! { None });
    let activation_check = if let Some(enabled) = enabled {
        quote::quote! { if !#enabled { continue; } }
    } else {
        quote::quote! {}
    };
    let handler_event_names = handler_events.iter().collect::<Vec<_>>();
    quote::quote! {
        #impl_block

        #end_callback

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #shim(
            world: &mut pulsar_scenedb::World,
            events: &pulsar_events::EventHub,
            delta_seconds: f32,
            previous: &std::collections::HashMap<pulsar_scenedb::Entity, pulsar_scenedb::Entity>,
            current: &mut std::collections::HashMap<pulsar_scenedb::Entity, pulsar_scenedb::Entity>,
            runtime: &mut pulsar_world_registry::ComponentRuntimeState,
        ) {
            // Every enabled instance ticks on its own; its owner object is the
            // actor whose event channel it uses.
            for (entity, (mut component, link)) in world.query::<(
                &mut #self_ty,
                &pulsar_world_registry::pulsar_scene_model::ComponentOwner,
            )>() {
                if !link.is_enabled() { continue; }
                #activation_check
                let owner = link.entity();
                let first_frame = !previous.contains_key(&entity);
                current.insert(entity, owner);
                let instance = pulsar_world_registry::ComponentInstanceKey {
                    component_type: pulsar_scenedb::component_id::<#self_ty>(),
                    entity,
                };
                runtime.subscribe_instance(instance, owner, events, &[#(#handler_event_names),*]);
                let mut context = pulsar_world_registry::ComponentContext::for_instance(entity, owner, events);
                if first_frame { #begin_call }
                for queued_event in runtime.take_events(instance) {
                    match queued_event.name.as_str() {
                        #(#handler_calls)*
                        _ => {}
                    }
                }
                component.tick(&mut context, delta_seconds);
            }
        }

        pulsar_world_registry::inventory::submit! {
            pulsar_world_registry::ComponentTickRegistration {
                type_name: stringify!(#self_ty),
                class_name: #class_name,
                handler_events: &[#(#handler_event_names),*],
                component_type: pulsar_scenedb::component_id::<#self_ty>,
                tick: #shim,
                end_play: #end_callback_value,
            }
        }
    }
}

fn find_method<'a>(impl_block: &'a ItemImpl, name: &str) -> Option<&'a syn::ImplItemFn> {
    impl_block.items.iter().find_map(|item| match item {
        syn::ImplItem::Fn(method) if method.sig.ident == name => Some(method),
        _ => None,
    })
}
