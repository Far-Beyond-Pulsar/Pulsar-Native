//! Native functions: Rust code scripts can call.
//!
//! A native has a stable qualified name and a full [`Signature`]. Modules
//! import natives by name and signature; the linker binds each import to
//! the [`NativeFn`] registered under that name and refuses a signature
//! mismatch, so a script can only ever call what is registered, with the
//! types it was verified against.
//!
//! Natives come from the engine (the standard library, reflected methods,
//! component methods and properties; see [`NativeRegistry::with_engine_natives`])
//! or from hot-reloadable native libraries (see [`crate::library`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use pulsar_reflection::methods::MethodFlags;
use pulsar_scenedb::{Entity, World};

use crate::error::ScriptError;
use crate::library::{LibraryId, ShadowLibrary};
use crate::module::{Param, Signature};
use crate::types::{ScriptValue, Type};
use crate::value::Value;

/// What a native can reach: the world, and the entity the calling script
/// instance is bound to.
pub struct Host<'w> {
    pub world: &'w mut World,
    pub entity: Entity,
}

impl<'w> Host<'w> {
    pub fn new(world: &'w mut World, entity: Entity) -> Self {
        Self { world, entity }
    }
}

/// A native's implementation. `args` holds exactly the signature's
/// parameters, already type-checked; `inout` parameters may be modified in
/// place and are written back to the caller. The return value must match
/// the signature's return type (the VM checks).
pub type NativeImpl =
    dyn Fn(&mut Host<'_>, &mut [Value]) -> Result<Value, ScriptError> + Send + Sync;

/// Where a native came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    Engine,
    Library(LibraryId),
}

/// A registered native function.
pub struct NativeFn {
    pub name: String,
    pub sig: Signature,
    /// `Some(ty)` when the first parameter is a receiver: the native is a
    /// method callable on a value or reference of type `ty`.
    pub receiver: Option<Type>,
    /// Parameter names for frontends (same length as `sig.params`).
    pub param_names: Vec<String>,
    pub doc: String,
    pub flags: MethodFlags,
    /// Free-form attributes (e.g. `category`) for frontends.
    pub attrs: Vec<(String, String)>,
    pub origin: Origin,
    // Field order matters: `call` (whose code may live in a library) must
    // drop before the library handle that keeps that code mapped.
    call: Box<NativeImpl>,
    library: Option<Arc<ShadowLibrary>>,
}

impl NativeFn {
    pub fn builder(name: impl Into<String>) -> NativeBuilder {
        NativeBuilder {
            name: name.into(),
            receiver: None,
            param_names: Vec::new(),
            doc: String::new(),
            flags: MethodFlags::NONE,
            attrs: Vec::new(),
        }
    }

    /// Call with arguments matching the signature.
    pub fn call(&self, host: &mut Host<'_>, args: &mut [Value]) -> Result<Value, ScriptError> {
        (self.call)(host, args)
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub(crate) fn attach_library(&mut self, id: LibraryId, library: Arc<ShadowLibrary>) {
        self.origin = Origin::Library(id);
        self.library = Some(library);
    }
}

impl fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{} [{:?}]", self.name, self.sig, self.origin)
    }
}

/// Builds a [`NativeFn`] from metadata and an implementation.
pub struct NativeBuilder {
    name: String,
    receiver: Option<Type>,
    param_names: Vec<String>,
    doc: String,
    flags: MethodFlags,
    attrs: Vec<(String, String)>,
}

impl NativeBuilder {
    pub fn doc(mut self, doc: impl Into<String>) -> Self {
        self.doc = doc.into();
        self
    }

    /// No side effects, same result for the same inputs.
    pub fn pure(mut self) -> Self {
        self.flags = MethodFlags::PURE;
        self
    }

    /// No side effects, but the result may vary (e.g. reads a clock).
    pub fn side_effect_free(mut self) -> Self {
        self.flags.side_effect_free = true;
        self
    }

    pub fn flags(mut self, flags: MethodFlags) -> Self {
        self.flags = flags;
        self
    }

    pub fn attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push((key.into(), value.into()));
        self
    }

    pub fn params<S: Into<String>>(mut self, names: impl IntoIterator<Item = S>) -> Self {
        self.param_names = names.into_iter().map(Into::into).collect();
        self
    }

    /// Mark the first parameter as the receiver of type `ty`.
    pub fn method_of(mut self, ty: Type) -> Self {
        self.receiver = Some(ty);
        self
    }

    /// Finish with a typed closure: `|x: f64| x.sin()`, or one taking the
    /// host first: `|host: &mut Host, e: Entity| host.world.is_alive(e)`.
    /// Arguments are [`ScriptValue`]s; the result is a `ScriptValue` or a
    /// `Result<ScriptValue, E: Display>`.
    pub fn build<M>(self, f: impl IntoNative<M>) -> NativeFn {
        let (params, ret, call) = f.into_native();
        self.build_raw(Signature::new(params, ret), call)
    }

    /// Finish with an explicit signature and an untyped implementation.
    pub fn build_raw(self, sig: Signature, call: Box<NativeImpl>) -> NativeFn {
        let mut param_names = self.param_names;
        param_names.resize_with(sig.params.len(), String::new);
        for (i, name) in param_names.iter_mut().enumerate() {
            if name.is_empty() {
                *name = format!("arg{i}");
            }
        }
        NativeFn {
            name: self.name,
            sig,
            receiver: self.receiver,
            param_names,
            doc: self.doc,
            flags: self.flags,
            attrs: self.attrs,
            origin: Origin::Engine,
            call,
            library: None,
        }
    }
}

/// The result of a typed native closure.
pub trait NativeReturn {
    fn script_type() -> Type;
    fn into_result(self) -> Result<Value, ScriptError>;
}

impl<T: ScriptValue> NativeReturn for T {
    fn script_type() -> Type {
        T::script_type()
    }
    fn into_result(self) -> Result<Value, ScriptError> {
        Ok(self.into_value())
    }
}

impl<T: ScriptValue, E: fmt::Display> NativeReturn for Result<T, E> {
    fn script_type() -> Type {
        T::script_type()
    }
    fn into_result(self) -> Result<Value, ScriptError> {
        self.map(T::into_value).map_err(|e| ScriptError::native(e.to_string()))
    }
}

/// A closure usable as a native. `M` distinguishes the closure shapes.
pub trait IntoNative<M> {
    fn into_native(self) -> (Vec<Param>, Type, Box<NativeImpl>);
}

/// Marker for closures that take `&mut Host` first.
pub struct WithHost<T>(std::marker::PhantomData<T>);

fn arg<A: ScriptValue>(args: &[Value], index: usize) -> Result<A, ScriptError> {
    A::from_value(&args[index]).ok_or_else(|| {
        ScriptError::native(format!(
            "argument {index}: expected {}, got {}",
            A::script_type(),
            args[index].kind()
        ))
    })
}

macro_rules! into_native {
    ($($arg:ident $idx:tt),*) => {
        impl<F, R, $($arg,)*> IntoNative<fn($($arg,)*) -> R> for F
        where
            F: Fn($($arg),*) -> R + Send + Sync + 'static,
            R: NativeReturn,
            $($arg: ScriptValue,)*
        {
            fn into_native(self) -> (Vec<Param>, Type, Box<NativeImpl>) {
                let params = vec![$(Param::new($arg::script_type())),*];
                #[allow(unused_variables)]
                let call = move |_host: &mut Host<'_>, args: &mut [Value]| {
                    self($(arg::<$arg>(args, $idx)?),*).into_result()
                };
                (params, R::script_type(), Box::new(call))
            }
        }

        impl<F, R, $($arg,)*> IntoNative<WithHost<fn($($arg,)*) -> R>> for F
        where
            F: Fn(&mut Host<'_>, $($arg),*) -> R + Send + Sync + 'static,
            R: NativeReturn,
            $($arg: ScriptValue,)*
        {
            fn into_native(self) -> (Vec<Param>, Type, Box<NativeImpl>) {
                let params = vec![$(Param::new($arg::script_type())),*];
                #[allow(unused_variables)]
                let call = move |host: &mut Host<'_>, args: &mut [Value]| {
                    self(host, $(arg::<$arg>(args, $idx)?),*).into_result()
                };
                (params, R::script_type(), Box::new(call))
            }
        }
    };
}

into_native!();
into_native!(A 0);
into_native!(A 0, B 1);
into_native!(A 0, B 1, C 2);
into_native!(A 0, B 1, C 2, D 3);
into_native!(A 0, B 1, C 2, D 3, E 4);
into_native!(A 0, B 1, C 2, D 3, E 4, G 5);

/// An engine native registered at link time (e.g. by pulsar_std's
/// `#[blueprint]` functions). Collected by
/// [`NativeRegistry::with_engine_natives`].
pub struct NativeRegistration {
    pub build: fn() -> NativeFn,
}

inventory::collect!(NativeRegistration);

/// Natives registered in bulk by another registry (e.g. the world
/// component registry's properties and methods). A native whose name is
/// already taken is skipped.
pub struct NativeProvider {
    pub natives: fn() -> Vec<NativeFn>,
}

inventory::collect!(NativeProvider);

/// A native with this name is already registered.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("native `{0}` is already registered")]
pub struct DuplicateNative(pub String);

/// Every native available for linking, by name.
///
/// The registry is an ordinary value owned by whoever runs scripts (the
/// script runtime). Its [`generation`](Self::generation) changes whenever
/// natives are added or removed; programs linked against an older
/// generation should be relinked.
#[derive(Default)]
pub struct NativeRegistry {
    natives: HashMap<String, Arc<NativeFn>>,
    generation: u64,
}

impl NativeRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The standard library, every script-visible reflected method,
    /// component method, component accessor and property, every
    /// [`NativeRegistration`], and every [`NativeProvider`]'s natives.
    pub fn with_engine_natives() -> Self {
        let mut registry = Self::new();
        crate::stdlib::register(&mut registry);
        crate::adapters::register(&mut registry);
        for registration in inventory::iter::<NativeRegistration> {
            if let Err(err) = registry.register((registration.build)()) {
                tracing::error!("script natives: {err}");
            }
        }
        for provider in inventory::iter::<NativeProvider> {
            for native in (provider.natives)() {
                if let Err(err) = registry.register(native) {
                    tracing::debug!("script natives: skipping provided {err}");
                }
            }
        }
        registry
    }

    pub fn register(&mut self, native: NativeFn) -> Result<(), DuplicateNative> {
        if self.natives.contains_key(&native.name) {
            return Err(DuplicateNative(native.name));
        }
        self.natives.insert(native.name.clone(), Arc::new(native));
        self.generation += 1;
        Ok(())
    }

    /// Remove every native with origin `origin`; returns their names.
    pub fn remove_origin(&mut self, origin: Origin) -> Vec<String> {
        let names: Vec<String> = self
            .natives
            .values()
            .filter(|n| n.origin == origin)
            .map(|n| n.name.clone())
            .collect();
        for name in &names {
            self.natives.remove(name);
        }
        if !names.is_empty() {
            self.generation += 1;
        }
        names
    }

    pub fn get(&self, name: &str) -> Option<&Arc<NativeFn>> {
        self.natives.get(name)
    }

    /// Every native, in no particular order.
    pub fn functions(&self) -> impl Iterator<Item = &Arc<NativeFn>> {
        self.natives.values()
    }

    /// Natives callable on a value or reference of type `ty` (its methods,
    /// accessors and properties).
    pub fn methods_for<'a>(&'a self, ty: &'a Type) -> impl Iterator<Item = &'a Arc<NativeFn>> {
        self.natives.values().filter(move |n| n.receiver.as_ref() == Some(ty))
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.natives.len()
    }

    pub fn is_empty(&self) -> bool {
        self.natives.is_empty()
    }
}
