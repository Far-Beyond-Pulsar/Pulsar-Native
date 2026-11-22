//! Memory reported by its owners: what a plugin holds, by item, in columns of
//! bytes it names, for tools (the Mission Control Memory Explorer's "Owners"
//! view and its exported report).
//!
//! The allocator's and the GPU's own reports say how much memory there is
//! and under which allocation name; only the owner of a resource knows what
//! it is for (a mesh's classic geometry, its stand-in, its streamed pages).
//! A plugin submits a [`MemoryOwnerRegistration`] from its root; the engine
//! names no kind.

/// One item of an owner's table: a label and its bytes, one per column.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryRow {
    pub label: String,
    pub bytes: Vec<u64>,
}

/// An owner's memory table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryTable {
    /// The owner's name ("Virtual geometry").
    pub owner: &'static str,
    /// What each column of bytes holds.
    pub columns: &'static [&'static str],
    pub rows: Vec<MemoryRow>,
}

/// A table of memory a plugin owns. `rows` is called at a tool's pace (about
/// once a second while the view is open), off the UI thread; it reads the
/// owner's live state and must not block on a frame.
pub struct MemoryOwnerRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    pub owner: &'static str,
    pub columns: &'static [&'static str],
    pub rows: fn() -> Vec<MemoryRow>,
}

impl pulsar_plugins::Contribution for MemoryOwnerRegistration {
    const CATEGORY: &'static str = "engine.memory_owner";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.owner.to_string()
    }
}

pulsar_plugins::registration!(MemoryOwnerRegistration);

/// Every enabled owner's table, by owner name.
pub fn memory_tables() -> Vec<MemoryTable> {
    let mut tables: Vec<MemoryTable> = pulsar_plugins::registered::<MemoryOwnerRegistration>()
        .map(|registration| MemoryTable {
            owner: registration.owner,
            columns: registration.columns,
            rows: (registration.rows)(),
        })
        .collect();
    tables.sort_by_key(|table| table.owner);
    tables
}
