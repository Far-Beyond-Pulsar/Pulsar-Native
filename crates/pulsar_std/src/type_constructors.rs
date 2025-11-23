//! Standard type constructors for the Pulsar type system
//!
//! This module wraps Rust's standard library types with Pulsar type aliases,
//! making them available in the visual type alias editor.

use pulsar_macros::blueprint_type;
use std::sync::{Arc, Mutex, RwLock};
use std::rc::Rc;
use std::cell::{Cell, RefCell};
use std::pin::Pin;
use std::borrow::Cow;
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::collections::{HashMap, HashSet, BTreeMap, BTreeSet};

// =============================================================================
// Smart Pointers
// =============================================================================

#[blueprint_type(params: 1, category: "Smart Pointers", description: "Heap-allocated value", unwrapped_name: "Box")]
pub type PBox<T> = Box<T>;

#[blueprint_type(params: 1, category: "Smart Pointers", description: "Thread-safe reference counting", unwrapped_name: "Arc")]
pub type PArc<T> = Arc<T>;

#[blueprint_type(params: 1, category: "Smart Pointers", description: "Single-threaded reference counting", unwrapped_name: "Rc")]
pub type PRc<T> = Rc<T>;

#[blueprint_type(params: 1, category: "Smart Pointers", description: "Pin a value in memory", unwrapped_name: "Pin")]
pub type PPin<T> = Pin<T>;

// =============================================================================
// Option & Result
// =============================================================================

#[blueprint_type(params: 1, category: "Option & Result", description: "Optional value", unwrapped_name: "Option")]
pub type POption<T> = Option<T>;

#[blueprint_type(params: 2, category: "Option & Result", description: "Success or error", unwrapped_name: "Result")]
pub type PResult<T, E> = Result<T, E>;

// =============================================================================
// Collections
// =============================================================================

#[blueprint_type(params: 1, category: "Collections", description: "Dynamic array", unwrapped_name: "Vec")]
pub type PVec<T> = Vec<T>;

#[blueprint_type(params: 2, category: "Collections", description: "Key-value map", unwrapped_name: "HashMap")]
pub type PHashMap<K, V> = HashMap<K, V>;

#[blueprint_type(params: 1, category: "Collections", description: "Unique values set", unwrapped_name: "HashSet")]
pub type PHashSet<T> = HashSet<T>;

#[blueprint_type(params: 2, category: "Collections", description: "Ordered map", unwrapped_name: "BTreeMap")]
pub type PBTreeMap<K, V> = BTreeMap<K, V>;

#[blueprint_type(params: 1, category: "Collections", description: "Ordered set", unwrapped_name: "BTreeSet")]
pub type PBTreeSet<T> = BTreeSet<T>;

// =============================================================================
// Interior Mutability
// =============================================================================

#[blueprint_type(params: 1, category: "Interior Mutability", description: "Shared mutable container", unwrapped_name: "Cell")]
pub type PCell<T> = Cell<T>;

#[blueprint_type(params: 1, category: "Interior Mutability", description: "Runtime borrow checking", unwrapped_name: "RefCell")]
pub type PRefCell<T> = RefCell<T>;

#[blueprint_type(params: 1, category: "Interior Mutability", description: "Mutual exclusion lock", unwrapped_name: "Mutex")]
pub type PMutex<T> = Mutex<T>;

#[blueprint_type(params: 1, category: "Interior Mutability", description: "Read-write lock", unwrapped_name: "RwLock")]
pub type PRwLock<T> = RwLock<T>;

// =============================================================================
// Other
// =============================================================================

// Note: Cow requires T: ToOwned, but we can't express trait bounds on type aliases
// Users should use Cow directly or create a custom wrapper
// #[blueprint_type(params: 1, category: "Other", description: "Clone-on-write smart pointer", unwrapped_name: "Cow")]
// pub type PCow<'a, T> = Cow<'a, T>;

#[blueprint_type(params: 1, category: "Other", description: "Zero-size type marker", unwrapped_name: "PhantomData")]
pub type PPhantomData<T> = PhantomData<T>;

#[blueprint_type(params: 1, category: "Other", description: "Manually managed memory", unwrapped_name: "ManuallyDrop")]
pub type PManuallyDrop<T> = ManuallyDrop<T>;