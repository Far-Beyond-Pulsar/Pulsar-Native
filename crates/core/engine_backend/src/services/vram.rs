//! Video memory: every allocation the engine's GPU devices hold, where it
//! sits, who owns it and how much of it holds live data.
//!
//! Read only when a tool asks (nothing runs per frame):
//!
//! - **Allocated** is each engine device's allocator report (wgpu's): every
//!   driver memory block and, inside it, every allocation by label, offset and
//!   size. Helio, SceneDB, the plugins and the UI all allocate through it, so
//!   nothing is missed.
//! - **Owner** is stamped on each allocation as it is made, through wgpu's
//!   allocation owners (`wgpu::wgt::allocation_owner`, a patch of the engine's
//!   wgpu): SceneDB names itself for everything it allocates (the hook
//!   `request_engine_device` installs), Helio names the graph node, pass or
//!   derivation that runs (`helio_core::vram`), the renderer host names each
//!   plugin's host part. An owner is a path (`Helio/shadow`). An allocation
//!   made outside every scope is [`UNSCOPED`]: the place still to give one.
//! - **Used** comes from owners, as [`VramClaim`]s: "the allocations labelled
//!   `X` are mine; of their `capacity` bytes, `used` hold live data". SceneDB
//!   reports its pools itself (`pulsar_scenedb::gpu::vram_use`); a plugin
//!   submits a [`VramProviderRegistration`] from its root. A claim also names
//!   the owner of an unscoped allocation of its label.
//!
//! Only video memory: process memory (the heap, the address space) is the
//! Memory Explorer's. The VRAM Map tool (`ui_vram_map`) draws this.

use std::collections::HashMap;

/// One allocation inside a driver memory block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VramAllocation {
    /// The owner path it was made under (`Helio/shadow`), `None` outside every
    /// owner scope.
    pub owner: Option<String>,
    /// The label the resource was created with (`<unnamed>` when none).
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

/// A block of memory the driver gave the allocator; allocations are carved
/// from it, sorted by offset. What none covers is free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VramBlock {
    pub size: u64,
    pub allocations: Vec<VramAllocation>,
}

impl VramBlock {
    pub fn allocated(&self) -> u64 {
        self.allocations.iter().map(|a| a.size).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VramDevice {
    /// Position in `device_loss::watched_devices`.
    pub index: usize,
    pub allocated: u64,
    pub reserved: u64,
    pub blocks: Vec<VramBlock>,
}

/// An owner's word on the allocations labelled `name`: `capacity` of their
/// bytes are what it sized them to, and `used` hold live data (`None` when the
/// owner cannot tell). `owner` names the unscoped ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VramClaim {
    pub name: String,
    pub owner: String,
    pub capacity: u64,
    pub used: Option<u64>,
}

/// A plugin's claims on video memory. `claims` runs at a tool's pace (about
/// once a second while the VRAM Map is open), off the UI thread; it reads the
/// owner's live state and must not wait for a frame.
pub struct VramProviderRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// Shown as the owner of what it claims ("Shadows").
    pub owner: &'static str,
    pub claims: fn(owner: &'static str, out: &mut Vec<VramClaim>),
}

impl pulsar_plugins::Contribution for VramProviderRegistration {
    const CATEGORY: &'static str = "engine.vram_provider";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.owner.to_string()
    }
}

pulsar_plugins::registration!(VramProviderRegistration);

/// What every engine device holds, and what the owners claim of it.
#[derive(Clone, Debug, Default)]
pub struct VramSnapshot {
    pub devices: Vec<VramDevice>,
    /// Devices whose backend gives no allocator report.
    pub unsupported: usize,
    pub claims: Vec<VramClaim>,
}

/// The claimed owner and use of every allocation of one label.
#[derive(Clone, Debug, PartialEq)]
pub struct NameUse {
    pub owner: String,
    /// Live bytes over capacity, `None` when no claim on the label can tell.
    pub used_fraction: Option<f64>,
}

/// The owner of allocations made outside every owner scope that no claim
/// names either.
pub const UNSCOPED: &str = "Unscoped";

impl VramSnapshot {
    /// Claimed owner and used fraction by label. Several claims on one label
    /// (one pool per scene, say) add up; the first claim's owner names it.
    pub fn name_use(&self) -> HashMap<&str, NameUse> {
        let mut totals: HashMap<&str, (&str, u64, u64, bool)> = HashMap::new();
        for claim in &self.claims {
            let entry = totals
                .entry(claim.name.as_str())
                .or_insert((claim.owner.as_str(), 0, 0, true));
            entry.1 += claim.capacity;
            match claim.used {
                Some(used) => entry.2 += used.min(claim.capacity),
                None => entry.3 = false,
            }
        }
        totals
            .into_iter()
            .map(|(name, (owner, capacity, used, known))| {
                let used_fraction =
                    (known && capacity > 0).then(|| used as f64 / capacity as f64);
                (
                    name,
                    NameUse {
                        owner: owner.to_owned(),
                        used_fraction,
                    },
                )
            })
            .collect()
    }

    /// The owner path of `allocation`: the scope it was made in, else the
    /// claim on its label, else [`UNSCOPED`].
    pub fn owner_of<'a>(
        allocation: &'a VramAllocation,
        uses: &'a HashMap<&str, NameUse>,
    ) -> &'a str {
        allocation
            .owner
            .as_deref()
            .or_else(|| uses.get(allocation.name.as_str()).map(|u| u.owner.as_str()))
            .unwrap_or(UNSCOPED)
    }

    pub fn allocated(&self) -> u64 {
        self.devices.iter().map(|d| d.allocated).sum()
    }

    pub fn reserved(&self) -> u64 {
        self.devices.iter().map(|d| d.reserved).sum()
    }
}

/// Convert one device's allocator report: blocks with their allocations in
/// offset order, each name taken apart into its owner and label.
pub fn device_from_report(index: usize, report: &wgpu::AllocatorReport) -> VramDevice {
    let blocks = report
        .blocks
        .iter()
        .map(|block| {
            let mut allocations: Vec<VramAllocation> = report
                .allocations
                .get(block.allocations.clone())
                .unwrap_or(&[])
                .iter()
                .map(|allocation| {
                    let (owner, label) = wgpu::wgt::allocation_owner::split(&allocation.name);
                    VramAllocation {
                        owner: owner.map(str::to_owned),
                        name: if label.is_empty() {
                            "<unnamed>".into()
                        } else {
                            label.to_owned()
                        },
                        offset: allocation.offset,
                        size: allocation.size,
                    }
                })
                .collect();
            allocations.sort_by_key(|allocation| allocation.offset);
            VramBlock {
                size: block.size,
                allocations,
            }
        })
        .collect();
    VramDevice {
        index,
        allocated: report.total_allocated_bytes,
        reserved: report.total_reserved_bytes,
        blocks,
    }
}

/// Read every engine device's allocator report and, with `usage`, every
/// owner's claims. Walks every allocation: call it off the UI and frame
/// threads, at a tool's pace.
pub fn snapshot(usage: bool) -> VramSnapshot {
    let mut snapshot = VramSnapshot::default();
    for (index, device) in super::device_loss::watched_devices().iter().enumerate() {
        match device.generate_allocator_report() {
            Some(report) => snapshot.devices.push(device_from_report(index, &report)),
            None => snapshot.unsupported += 1,
        }
    }
    if usage {
        for pool in pulsar_scenedb::gpu::vram_use() {
            snapshot.claims.push(VramClaim {
                name: pool.label,
                owner: "SceneDB".into(),
                capacity: pool.capacity_bytes,
                used: Some(pool.used_bytes),
            });
        }
        for provider in pulsar_plugins::registered::<VramProviderRegistration>() {
            (provider.claims)(provider.owner, &mut snapshot.claims);
        }
    }
    snapshot
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::wgt::{AllocationReport, AllocatorReport, MemoryBlockReport};

    const MB: u64 = 1024 * 1024;

    fn allocation(name: &str, offset: u64, size: u64) -> AllocationReport {
        AllocationReport {
            name: name.into(),
            offset,
            size,
        }
    }

    #[test]
    fn blocks_keep_their_allocations_in_offset_order_and_names_split_into_owner_and_label() {
        let scoped = format!("Helio/shadow{}atlas", wgpu::wgt::allocation_owner::SEPARATOR);
        let report = AllocatorReport {
            allocations: vec![
                allocation(&scoped, 64 * MB, 32 * MB),
                allocation("a", 0, 64 * MB),
                allocation("", 0, MB),
            ],
            blocks: vec![
                MemoryBlockReport {
                    size: 256 * MB,
                    allocations: 0..2,
                },
                MemoryBlockReport {
                    size: 8 * MB,
                    allocations: 2..3,
                },
            ],
            total_allocated_bytes: 97 * MB,
            total_reserved_bytes: 264 * MB,
        };
        let device = device_from_report(0, &report);
        assert_eq!(device.blocks[0].allocations[0].name, "a");
        assert_eq!(device.blocks[0].allocations[0].owner, None);
        let atlas = &device.blocks[0].allocations[1];
        assert_eq!((atlas.owner.as_deref(), atlas.name.as_str()), (Some("Helio/shadow"), "atlas"));
        assert_eq!(device.blocks[0].allocated(), 96 * MB);
        assert_eq!(device.blocks[1].allocations[0].name, "<unnamed>");
        assert_eq!(device.reserved, 264 * MB);
    }

    #[test]
    fn claims_on_one_label_add_up_and_name_only_unscoped_allocations() {
        let claim = |name: &str, capacity, used| VramClaim {
            name: name.into(),
            owner: "SceneDB".into(),
            capacity,
            used,
        };
        let snapshot = VramSnapshot {
            claims: vec![
                claim("pool", 100, Some(25)),
                claim("pool", 100, Some(75)),
                claim("atlas", 100, None),
            ],
            ..Default::default()
        };
        let uses = snapshot.name_use();
        assert_eq!(uses["pool"].used_fraction, Some(0.5));
        assert_eq!(uses["atlas"].used_fraction, None);
        let unscoped = VramAllocation {
            owner: None,
            name: "pool".into(),
            offset: 0,
            size: 1,
        };
        assert_eq!(VramSnapshot::owner_of(&unscoped, &uses), "SceneDB");
        let scoped = VramAllocation {
            owner: Some("Helio/gi".into()),
            ..unscoped.clone()
        };
        assert_eq!(VramSnapshot::owner_of(&scoped, &uses), "Helio/gi");
        let nobody = VramAllocation {
            name: "x".into(),
            ..unscoped
        };
        assert_eq!(VramSnapshot::owner_of(&nobody, &uses), UNSCOPED);
    }
}
