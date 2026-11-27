//! The map's tree, built from a [`VramSnapshot`], and its placement.
//!
//! Two arrangements of the same allocations:
//!
//! - **By owner**: device, then the owner path each allocation was made under
//!   (`Helio` › `shadow`; allocations made outside every owner scope under
//!   `Unscoped`), then label, then each allocation when a label has several.
//!   The free space inside the driver's blocks is a box of its own: memory the
//!   process holds and nothing uses.
//! - **By block**: device, then each driver memory block, then what fills it
//!   in address order, free gaps included. This is the physical layout:
//!   fragmentation shows as grey between allocations.
//!
//! A box's area is its allocated bytes; its used fraction comes from the
//! owners' claims (`engine_backend::services::vram`).

use std::collections::{BTreeMap, HashMap};

use engine_backend::services::vram::{NameUse, VramDevice, VramSnapshot};

use super::layout::{squarify, strip, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrangement {
    ByOwner,
    ByBlock,
}

/// What a box is, for its colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tint {
    /// Coloured by this owner path (its first part picks the hue).
    Owner(String),
    /// Memory held and holding nothing.
    Free,
    /// A device, a block: only a frame.
    Frame,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub label: String,
    pub bytes: u64,
    /// Live bytes over allocated bytes; `None` when no owner can tell.
    pub used: Option<f64>,
    pub tint: Tint,
    /// Allocations under this box.
    pub count: usize,
    /// Children are laid out in order (addresses), not squarified.
    pub ordered: bool,
    pub children: Vec<Node>,
}

impl Node {
    fn group(label: String, tint: Tint, ordered: bool, children: Vec<Node>) -> Node {
        let bytes = children.iter().map(|c| c.bytes).sum();
        let count = children.iter().map(|c| c.count).sum();
        // A group's use is the bytes-weighted use of its children, known only
        // when every child's is.
        let used = children
            .iter()
            .try_fold(0.0f64, |acc, c| c.used.map(|u| acc + u * c.bytes as f64))
            .filter(|_| bytes > 0)
            .map(|live| live / bytes as f64);
        Node {
            label,
            bytes,
            used,
            tint,
            count,
            ordered,
            children,
        }
    }

    fn leaf(label: String, bytes: u64, used: Option<f64>, tint: Tint) -> Node {
        let count = usize::from(!matches!(tint, Tint::Free));
        Node {
            label,
            bytes,
            used,
            tint,
            count,
            ordered: false,
            children: Vec::new(),
        }
    }
}

pub const FREE_IN_BLOCKS: &str = "Free in driver blocks";

/// The tree of `snapshot` in `arrangement`.
pub fn build(snapshot: &VramSnapshot, arrangement: Arrangement) -> Node {
    let uses = snapshot.name_use();
    let devices: Vec<Node> = snapshot
        .devices
        .iter()
        .map(|device| {
            let label = format!("Device {}", device.index);
            let children = match arrangement {
                Arrangement::ByOwner => by_owner(device, &uses),
                Arrangement::ByBlock => by_block(device, &uses),
            };
            Node::group(label, Tint::Frame, false, children)
        })
        .collect();
    match devices.len() {
        1 => devices.into_iter().next().unwrap_or_else(empty),
        _ => Node::group("All devices".into(), Tint::Frame, false, devices),
    }
}

fn empty() -> Node {
    Node::group(String::new(), Tint::Frame, false, Vec::new())
}

/// One level of the owner path: deeper owners, and the labels allocated here.
#[derive(Default)]
struct OwnerLevel<'a> {
    owners: BTreeMap<&'a str, OwnerLevel<'a>>,
    labels: BTreeMap<&'a str, Vec<u64>>,
}

impl<'a> OwnerLevel<'a> {
    fn into_children(self, path: &str, uses: &HashMap<&str, NameUse>) -> Vec<Node> {
        let mut children: Vec<Node> = self
            .owners
            .into_iter()
            .map(|(segment, level)| {
                let path = if path.is_empty() {
                    segment.to_owned()
                } else {
                    format!("{path}/{segment}")
                };
                let children = level.into_children(&path, uses);
                Node::group(segment.to_owned(), Tint::Owner(path), false, children)
            })
            .collect();
        let tint = Tint::Owner(path.to_owned());
        children.extend(self.labels.into_iter().map(|(label, sizes)| {
            let used = uses.get(label).and_then(|u| u.used_fraction);
            if sizes.len() == 1 {
                return Node::leaf(label.to_owned(), sizes[0], used, tint.clone());
            }
            let parts = sizes
                .iter()
                .map(|&size| Node::leaf(label.to_owned(), size, used, tint.clone()))
                .collect();
            Node::group(format!("{label} ×{}", sizes.len()), tint.clone(), false, parts)
        }));
        children
    }
}

fn by_owner(device: &VramDevice, uses: &HashMap<&str, NameUse>) -> Vec<Node> {
    let mut root = OwnerLevel::default();
    for allocation in device.blocks.iter().flat_map(|b| &b.allocations) {
        let mut level = &mut root;
        for segment in VramSnapshot::owner_of(allocation, uses).split('/') {
            level = level.owners.entry(segment).or_default();
        }
        level
            .labels
            .entry(allocation.name.as_str())
            .or_default()
            .push(allocation.size);
    }
    let mut children = root.into_children("", uses);
    let free: u64 = device
        .blocks
        .iter()
        .map(|b| b.size.saturating_sub(b.allocated()))
        .sum();
    if free > 0 {
        children.push(Node::leaf(FREE_IN_BLOCKS.into(), free, Some(0.0), Tint::Free));
    }
    children
}

fn by_block(device: &VramDevice, uses: &HashMap<&str, NameUse>) -> Vec<Node> {
    device
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let mut parts = Vec::with_capacity(block.allocations.len() * 2 + 1);
            let mut at = 0u64;
            for allocation in &block.allocations {
                if allocation.offset > at {
                    parts.push(Node::leaf("free".into(), allocation.offset - at, Some(0.0), Tint::Free));
                }
                let owner = VramSnapshot::owner_of(allocation, uses);
                let used = uses.get(allocation.name.as_str()).and_then(|u| u.used_fraction);
                parts.push(Node::leaf(
                    allocation.name.clone(),
                    allocation.size,
                    used,
                    Tint::Owner(owner.to_owned()),
                ));
                at = at.max(allocation.offset + allocation.size);
            }
            if block.size > at {
                parts.push(Node::leaf("free".into(), block.size - at, Some(0.0), Tint::Free));
            }
            Node::group(format!("Block {index}"), Tint::Frame, true, parts)
        })
        .collect()
}

/// A placed box, in layout units.
#[derive(Clone, Debug)]
pub struct Placed {
    pub rect: Rect,
    /// The box this one sits in (`None` for the root's children).
    pub parent: Option<usize>,
    pub label: String,
    pub bytes: u64,
    pub used: Option<f64>,
    pub tint: Tint,
    pub count: usize,
    pub leaf: bool,
    /// The header strip a group's label goes in, when it got one.
    pub header: bool,
}

/// Gap between a group's frame and its children, in layout units.
pub const PAD: f32 = 2.0;
/// A group's header strip, in layout units, when the group is large enough.
pub const HEADER: f32 = 15.0;

/// Place `root`'s children in `rect`, depth first; parents come before their
/// children.
pub fn place(root: &Node, rect: Rect) -> Vec<Placed> {
    let mut out = Vec::new();
    place_children(root, rect, None, &mut out);
    out
}

fn place_children(node: &Node, rect: Rect, parent: Option<usize>, out: &mut Vec<Placed>) {
    let sizes: Vec<f64> = node.children.iter().map(|c| c.bytes as f64).collect();
    let rects = if node.ordered {
        strip(&sizes, rect)
    } else {
        squarify(&sizes, rect)
    };
    for (child, rect) in node.children.iter().zip(rects) {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            continue;
        }
        let leaf = child.children.is_empty();
        let header = !leaf && rect.w > 70.0 && rect.h > 3.0 * HEADER;
        let index = out.len();
        out.push(Placed {
            rect,
            parent,
            label: child.label.clone(),
            bytes: child.bytes,
            used: child.used,
            tint: child.tint.clone(),
            count: child.count,
            leaf,
            header,
        });
        if !leaf {
            let inner = rect.inset(if header { HEADER } else { PAD }, PAD);
            if let Some(inner) = inner {
                place_children(child, inner, Some(index), out);
            }
        }
    }
}

/// The deepest box at `(x, y)`.
pub fn hit(placed: &[Placed], x: f32, y: f32) -> Option<usize> {
    // Children come after their parents, so the last hit is the deepest.
    placed.iter().rposition(|p| p.rect.contains(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::services::vram::{VramAllocation, VramBlock, VramClaim, UNSCOPED};

    const MB: u64 = 1024 * 1024;

    fn alloc(owner: Option<&str>, name: &str, offset: u64, size: u64) -> VramAllocation {
        VramAllocation {
            owner: owner.map(str::to_owned),
            name: name.into(),
            offset,
            size,
        }
    }

    fn snapshot() -> VramSnapshot {
        VramSnapshot {
            devices: vec![VramDevice {
                index: 0,
                allocated: 128 * MB,
                reserved: 256 * MB,
                blocks: vec![VramBlock {
                    size: 256 * MB,
                    allocations: vec![
                        alloc(Some("SceneDB"), "Scene Texture", 0, 32 * MB),
                        alloc(Some("SceneDB"), "Scene Texture", 32 * MB, 16 * MB),
                        alloc(Some("Helio/shadow"), "atlas", 48 * MB, 16 * MB),
                        alloc(Some("SceneDB"), "pool", 64 * MB, 64 * MB),
                        alloc(None, "stray", 128 * MB, MB),
                    ],
                }],
            }],
            unsupported: 0,
            claims: vec![VramClaim {
                name: "pool".into(),
                owner: "SceneDB".into(),
                capacity: 64 * MB,
                used: Some(16 * MB),
            }],
        }
    }

    #[test]
    fn by_owner_nests_owner_paths_labels_and_free_space() {
        let tree = build(&snapshot(), Arrangement::ByOwner);
        assert_eq!(tree.bytes, 256 * MB);
        let labels: Vec<&str> = tree.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Helio", "SceneDB", UNSCOPED, FREE_IN_BLOCKS]);
        let helio = &tree.children[0];
        assert_eq!(helio.children[0].label, "shadow");
        assert_eq!(helio.children[0].children[0].label, "atlas");
        assert_eq!(helio.children[0].tint, Tint::Owner("Helio/shadow".into()));
        let scenedb = &tree.children[1];
        let labels: Vec<&str> = scenedb.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Scene Texture ×2", "pool"]);
        assert_eq!(scenedb.children[1].used, Some(0.25));
        assert_eq!(scenedb.used, None, "the textures' use is not reported");
        assert_eq!(tree.children[3].bytes, 127 * MB);
    }

    #[test]
    fn by_block_keeps_address_order_with_the_gaps() {
        let tree = build(&snapshot(), Arrangement::ByBlock);
        let block = &tree.children[0];
        assert!(block.ordered);
        let labels: Vec<&str> = block.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Scene Texture", "Scene Texture", "atlas", "pool", "stray", "free"]);
        assert_eq!(block.children[2].tint, Tint::Owner("Helio/shadow".into()));
        assert_eq!(block.children[4].tint, Tint::Owner(UNSCOPED.into()));
        assert_eq!(block.bytes, 256 * MB);
    }

    #[test]
    fn placed_children_sit_inside_their_parents_and_hits_find_the_deepest() {
        let tree = build(&snapshot(), Arrangement::ByOwner);
        let placed = place(&tree, Rect::new(0.0, 0.0, 800.0, 500.0));
        for p in &placed {
            if let Some(parent) = p.parent {
                let outer = placed[parent].rect;
                assert!(p.rect.x >= outer.x - 1e-3 && p.rect.x + p.rect.w <= outer.x + outer.w + 1e-3);
            }
        }
        let leaf = placed.iter().position(|p| p.leaf && p.label == "pool").unwrap();
        let r = placed[leaf].rect;
        assert_eq!(hit(&placed, r.x + r.w / 2.0, r.y + r.h / 2.0), Some(leaf));
    }
}
