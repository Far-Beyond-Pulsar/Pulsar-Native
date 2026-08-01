use crate::{ContentHash, NodeState, PageKey, TerrainNodeSummary};
use thiserror::Error;

const HIERARCHY_MAGIC: &[u8; 8] = b"PTHIER02";

#[derive(Clone, Debug, PartialEq, Eq)]
enum TreeNode {
    Leaf {
        state: NodeState,
        summary: TerrainNodeSummary,
    },
    Branch {
        summary: TerrainNodeSummary,
        children: Box<[TreeNode; 8]>,
    },
}

impl TreeNode {
    const fn summary(&self) -> TerrainNodeSummary {
        match self {
            Self::Leaf { summary, .. } | Self::Branch { summary, .. } => *summary,
        }
    }
}

/// A mutable sparse brick hierarchy covering a centered cube of LOD0 pages.
/// Splitting one address allocates only eight siblings per traversed level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparseBrickTree {
    root_lod: u8,
    root_min_page: [i64; 3],
    root: TreeNode,
}

impl SparseBrickTree {
    pub fn centered(root_lod: u8, state: NodeState) -> Result<Self, HierarchyError> {
        let summary = TerrainNodeSummary::for_state(&state, 0);
        Self::centered_with_summary(root_lod, state, summary)
    }

    pub fn centered_with_summary(
        root_lod: u8,
        state: NodeState,
        summary: TerrainNodeSummary,
    ) -> Result<Self, HierarchyError> {
        if root_lod == 0 || root_lod > 62 || state == NodeState::Branch {
            return Err(HierarchyError::InvalidRoot);
        }
        validate_leaf_summary(&state, summary)?;
        let half = 1_i64 << (root_lod - 1);
        Ok(Self {
            root_lod,
            root_min_page: [-half; 3],
            root: TreeNode::Leaf { state, summary },
        })
    }

    pub fn root_state(&self) -> NodeState {
        match &self.root {
            TreeNode::Leaf { state, .. } => state.clone(),
            TreeNode::Branch { .. } => NodeState::Branch,
        }
    }

    pub const fn root_summary(&self) -> TerrainNodeSummary {
        self.root.summary()
    }

    pub fn root_lod(&self) -> u8 {
        self.root_lod
    }

    /// Exact O(1) root replacement, including whole-planet deletion.
    pub fn set_root(&mut self, state: NodeState) -> Result<(), HierarchyError> {
        let summary = TerrainNodeSummary::for_state(&state, 0);
        self.set_root_with_summary(state, summary)
    }

    pub fn set_root_with_summary(
        &mut self,
        state: NodeState,
        summary: TerrainNodeSummary,
    ) -> Result<(), HierarchyError> {
        if state == NodeState::Branch {
            return Err(HierarchyError::BranchIsInternal);
        }
        validate_leaf_summary(&state, summary)?;
        self.root = TreeNode::Leaf { state, summary };
        Ok(())
    }

    pub fn set(&mut self, key: PageKey, state: NodeState) -> Result<(), HierarchyError> {
        let summary = TerrainNodeSummary::for_state(&state, 0);
        self.set_with_summary(key, state, summary, |_, inherited, _| inherited)
    }

    pub fn set_with_summary<F>(
        &mut self,
        key: PageKey,
        state: NodeState,
        summary: TerrainNodeSummary,
        summarize_split_leaf: F,
    ) -> Result<(), HierarchyError>
    where
        F: Fn(PageKey, TerrainNodeSummary, &NodeState) -> TerrainNodeSummary,
    {
        if state == NodeState::Branch {
            return Err(HierarchyError::BranchIsInternal);
        }
        validate_leaf_summary(&state, summary)?;
        let path = self.path_for(key)?;
        set_recursive(
            &mut self.root,
            None,
            &path,
            state,
            summary,
            self.root_lod,
            &summarize_split_leaf,
        );
        Ok(())
    }

    pub fn resolve(&self, key: PageKey) -> Result<NodeState, HierarchyError> {
        let path = self.path_for(key)?;
        let mut node = &self.root;
        for step in path {
            match node {
                TreeNode::Leaf { state, .. } => return Ok(state.clone()),
                TreeNode::Branch { children, .. } => node = &children[step.child],
            }
        }
        Ok(match node {
            TreeNode::Leaf { state, .. } => state.clone(),
            TreeNode::Branch { .. } => NodeState::Branch,
        })
    }

    /// Resolve canonical node metadata. If a target lies below a compressed
    /// leaf, `summarize_descendant` derives the target-specific conservative
    /// summary without materializing hierarchy nodes.
    pub fn resolve_with_summary<F>(
        &self,
        key: PageKey,
        summarize_descendant: F,
    ) -> Result<(NodeState, TerrainNodeSummary), HierarchyError>
    where
        F: Fn(PageKey, TerrainNodeSummary, &NodeState) -> TerrainNodeSummary,
    {
        let path = self.path_for(key)?;
        let mut node = &self.root;
        for step in &path {
            match node {
                TreeNode::Leaf { state, summary } => {
                    return Ok((state.clone(), summarize_descendant(key, *summary, state)));
                }
                TreeNode::Branch { children, .. } => node = &children[step.child],
            }
        }
        Ok(match node {
            TreeNode::Leaf { state, summary } => (state.clone(), *summary),
            TreeNode::Branch { summary, .. } => (NodeState::Branch, *summary),
        })
    }

    pub fn node_count(&self) -> usize {
        count_nodes(&self.root)
    }

    pub fn max_summary_sequence(&self) -> u64 {
        max_summary_sequence(&self.root)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(40 + self.node_count() * 2);
        output.extend_from_slice(HIERARCHY_MAGIC);
        output.push(self.root_lod);
        output.extend_from_slice(&[0; 7]);
        for axis in self.root_min_page {
            output.extend_from_slice(&axis.to_le_bytes());
        }
        encode_node(&self.root, &mut output);
        output
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HierarchyError> {
        if bytes.len() < 65
            || bytes.get(..8) != Some(HIERARCHY_MAGIC)
            || bytes.get(9..16) != Some(&[0; 7])
        {
            return Err(HierarchyError::Codec);
        }
        let root_lod = bytes[8];
        if root_lod == 0 || root_lod > 62 {
            return Err(HierarchyError::Codec);
        }
        let mut root_min_page = [0_i64; 3];
        for (axis, value) in root_min_page.iter_mut().enumerate() {
            let offset = 16 + axis * 8;
            *value = i64::from_le_bytes(
                bytes
                    .get(offset..offset + 8)
                    .ok_or(HierarchyError::Codec)?
                    .try_into()
                    .map_err(|_| HierarchyError::Codec)?,
            );
        }
        let expected_min = -(1_i64 << (root_lod - 1));
        if root_min_page != [expected_min; 3] {
            return Err(HierarchyError::Codec);
        }
        let mut cursor = 40;
        let root = decode_node(bytes, &mut cursor, 0, root_lod)?;
        if cursor != bytes.len() {
            return Err(HierarchyError::Codec);
        }
        Ok(Self {
            root_lod,
            root_min_page,
            root,
        })
    }

    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of(&self.encode())
    }

    fn path_for(&self, key: PageKey) -> Result<Vec<PathStep>, HierarchyError> {
        if key.lod >= self.root_lod {
            return Err(HierarchyError::LodOutsideRoot(key.lod));
        }
        let target_min = key.lod0_min().ok_or(HierarchyError::CoordinateOverflow)?;
        let target_size = 1_i64 << key.lod;
        let root_size = 1_i64 << self.root_lod;
        for axis in 0..3 {
            let relative = target_min[axis]
                .checked_sub(self.root_min_page[axis])
                .ok_or(HierarchyError::CoordinateOverflow)?;
            if relative < 0 || relative.saturating_add(target_size) > root_size {
                return Err(HierarchyError::OutsideRoot(key));
            }
        }

        let depth = usize::from(self.root_lod - key.lod);
        let relative = [
            target_min[0] - self.root_min_page[0],
            target_min[1] - self.root_min_page[1],
            target_min[2] - self.root_min_page[2],
        ];
        let mut path = Vec::with_capacity(depth);
        for step in 0..depth {
            let bit = u32::from(self.root_lod - 1 - step as u8);
            let x = ((relative[0] >> bit) & 1) as usize;
            let y = ((relative[1] >> bit) & 1) as usize;
            let z = ((relative[2] >> bit) & 1) as usize;
            let lod = self.root_lod - 1 - step as u8;
            let scale = 1_i64 << lod;
            path.push(PathStep {
                child: x | (y << 1) | (z << 2),
                key: PageKey::new(lod, target_min.map(|axis| axis.div_euclid(scale))),
            });
        }
        Ok(path)
    }
}

#[derive(Clone, Copy)]
struct PathStep {
    child: usize,
    key: PageKey,
}

fn set_recursive<F>(
    node: &mut TreeNode,
    node_key: Option<PageKey>,
    path: &[PathStep],
    state: NodeState,
    summary: TerrainNodeSummary,
    root_lod: u8,
    summarize_split_leaf: &F,
) where
    F: Fn(PageKey, TerrainNodeSummary, &NodeState) -> TerrainNodeSummary,
{
    if path.is_empty() {
        *node = TreeNode::Leaf { state, summary };
        return;
    }
    if let TreeNode::Leaf {
        state: previous,
        summary: inherited,
    } = node
    {
        let previous = previous.clone();
        let inherited = *inherited;
        let keys = child_keys(node_key, root_lod);
        let children = Box::new(std::array::from_fn(|index| TreeNode::Leaf {
            state: previous.clone(),
            summary: summarize_split_leaf(keys[index], inherited, &previous),
        }));
        let summary = union_children(&children);
        *node = TreeNode::Branch { summary, children };
    }
    let TreeNode::Branch {
        summary: node_summary,
        children,
    } = node
    else {
        unreachable!();
    };
    let step = path[0];
    set_recursive(
        &mut children[step.child],
        Some(step.key),
        &path[1..],
        state,
        summary,
        root_lod,
        summarize_split_leaf,
    );

    let first = match &children[0] {
        TreeNode::Leaf { state, .. } => state.clone(),
        TreeNode::Branch { .. } => {
            *node_summary = union_children(children);
            return;
        }
    };
    if children
        .iter()
        .all(|child| matches!(child, TreeNode::Leaf { state, .. } if *state == first))
    {
        *node = TreeNode::Leaf {
            state: first,
            summary: union_children(children),
        };
    } else {
        *node_summary = union_children(children);
    }
}

fn child_keys(parent: Option<PageKey>, root_lod: u8) -> [PageKey; 8] {
    let lod = parent.map_or(root_lod - 1, |key| key.lod - 1);
    std::array::from_fn(|index| {
        let offsets = [
            (index & 1) as i64,
            ((index >> 1) & 1) as i64,
            ((index >> 2) & 1) as i64,
        ];
        let page_xyz = parent.map_or_else(
            || offsets.map(|offset| offset - 1),
            |key| {
                std::array::from_fn(|axis| {
                    key.page_xyz[axis]
                        .checked_mul(2)
                        .and_then(|value| value.checked_add(offsets[axis]))
                        .expect("a validated hierarchy path cannot overflow")
                })
            },
        );
        PageKey::new(lod, page_xyz)
    })
}

fn union_children(children: &[TreeNode; 8]) -> TerrainNodeSummary {
    children[1..]
        .iter()
        .fold(children[0].summary(), |summary, child| {
            summary.union(child.summary())
        })
}

fn count_nodes(node: &TreeNode) -> usize {
    match node {
        TreeNode::Leaf { .. } => 1,
        TreeNode::Branch { children, .. } => 1 + children.iter().map(count_nodes).sum::<usize>(),
    }
}

fn max_summary_sequence(node: &TreeNode) -> u64 {
    match node {
        TreeNode::Leaf { summary, .. } => summary.through_sequence(),
        TreeNode::Branch { summary, children } => children
            .iter()
            .fold(summary.through_sequence(), |latest, child| {
                latest.max(max_summary_sequence(child))
            }),
    }
}

fn encode_node(node: &TreeNode, output: &mut Vec<u8>) {
    match node {
        TreeNode::Leaf {
            state: NodeState::Air,
            summary,
        } => {
            output.push(0);
            encode_summary(*summary, output);
        }
        TreeNode::Leaf {
            state: NodeState::Solid(material),
            summary,
        } => {
            output.push(1);
            encode_summary(*summary, output);
            output.push(*material);
        }
        TreeNode::Leaf {
            state: NodeState::Procedural(hash),
            summary,
        } => {
            output.push(2);
            encode_summary(*summary, output);
            output.extend_from_slice(&hash.0);
        }
        TreeNode::Branch { summary, children } => {
            output.push(3);
            encode_summary(*summary, output);
            for child in children.iter() {
                encode_node(child, output);
            }
        }
        TreeNode::Leaf {
            state: NodeState::Page(hash),
            summary,
        } => {
            output.push(4);
            encode_summary(*summary, output);
            output.extend_from_slice(&hash.0);
        }
        TreeNode::Leaf {
            state: NodeState::Branch,
            ..
        } => unreachable!("Branch is stored structurally"),
    }
}

fn encode_summary(summary: TerrainNodeSummary, output: &mut Vec<u8>) {
    output.extend_from_slice(&summary.min_density().to_le_bytes());
    output.extend_from_slice(&summary.max_density().to_le_bytes());
    output.extend_from_slice(&summary.geometric_error_lod0_cells().to_le_bytes());
    output.extend_from_slice(&summary.through_sequence().to_le_bytes());
}

fn decode_node(
    bytes: &[u8],
    cursor: &mut usize,
    depth: u8,
    root_lod: u8,
) -> Result<TreeNode, HierarchyError> {
    let tag = *bytes.get(*cursor).ok_or(HierarchyError::Codec)?;
    *cursor += 1;
    let summary = decode_summary(bytes, cursor)?;
    match tag {
        0 => decode_leaf(NodeState::Air, summary),
        1 => {
            let material = *bytes.get(*cursor).ok_or(HierarchyError::Codec)?;
            *cursor += 1;
            decode_leaf(NodeState::Solid(material), summary)
        }
        2 | 4 => {
            let hash = ContentHash(
                bytes
                    .get(*cursor..*cursor + 32)
                    .ok_or(HierarchyError::Codec)?
                    .try_into()
                    .map_err(|_| HierarchyError::Codec)?,
            );
            *cursor += 32;
            decode_leaf(
                if tag == 2 {
                    NodeState::Procedural(hash)
                } else {
                    NodeState::Page(hash)
                },
                summary,
            )
        }
        3 if depth < root_lod => {
            let mut children = Vec::with_capacity(8);
            for _ in 0..8 {
                children.push(decode_node(bytes, cursor, depth + 1, root_lod)?);
            }
            let children: Box<[TreeNode; 8]> =
                Box::new(children.try_into().map_err(|_| HierarchyError::Codec)?);
            if union_children(&children) != summary {
                return Err(HierarchyError::Codec);
            }
            Ok(TreeNode::Branch { summary, children })
        }
        _ => Err(HierarchyError::Codec),
    }
}

fn decode_summary(bytes: &[u8], cursor: &mut usize) -> Result<TerrainNodeSummary, HierarchyError> {
    let min_density = i32::from_le_bytes(read_array(bytes, *cursor)?);
    let max_density = i32::from_le_bytes(read_array(bytes, *cursor + 4)?);
    let geometric_error_lod0_cells = u64::from_le_bytes(read_array(bytes, *cursor + 8)?);
    let through_sequence = u64::from_le_bytes(read_array(bytes, *cursor + 16)?);
    *cursor += 24;
    TerrainNodeSummary::new(
        min_density,
        max_density,
        geometric_error_lod0_cells,
        through_sequence,
    )
    .ok_or(HierarchyError::Codec)
}

fn decode_leaf(state: NodeState, summary: TerrainNodeSummary) -> Result<TreeNode, HierarchyError> {
    validate_leaf_summary(&state, summary).map_err(|_| HierarchyError::Codec)?;
    Ok(TreeNode::Leaf { state, summary })
}

fn validate_leaf_summary(
    state: &NodeState,
    summary: TerrainNodeSummary,
) -> Result<(), HierarchyError> {
    let valid = match state {
        NodeState::Air => summary.is_uniform_air(),
        NodeState::Solid(_) => summary.is_uniform_solid(),
        NodeState::Procedural(_) | NodeState::Page(_) => true,
        NodeState::Branch => false,
    };
    if valid {
        Ok(())
    } else {
        Err(HierarchyError::InvalidSummary)
    }
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], HierarchyError> {
    bytes
        .get(offset..offset + N)
        .ok_or(HierarchyError::Codec)?
        .try_into()
        .map_err(|_| HierarchyError::Codec)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HierarchyError {
    #[error("root LOD must be 1..=62 and root state must be uniform")]
    InvalidRoot,
    #[error("Branch is an internal state and cannot be assigned directly")]
    BranchIsInternal,
    #[error("LOD{0} is not a descendant of the root")]
    LodOutsideRoot(u8),
    #[error("page {0:?} is outside the centered root cube")]
    OutsideRoot(PageKey),
    #[error("page coordinate overflow")]
    CoordinateOverflow,
    #[error("invalid canonical sparse-hierarchy encoding")]
    Codec,
    #[error("terrain node summary contradicts its canonical node state")]
    InvalidSummary,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_coordinates_resolve_and_uniform_siblings_collapse() {
        let mut tree = SparseBrickTree::centered(4, NodeState::Air).unwrap();
        let parent = PageKey::new(1, [-1, 0, 0]);
        for z in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    tree.set(PageKey::new(0, [-2 + x, y, z]), NodeState::Solid(4))
                        .unwrap();
                }
            }
        }
        assert_eq!(tree.resolve(parent).unwrap(), NodeState::Solid(4));
        assert!(tree.node_count() < 1 + 8 * 4);
    }

    #[test]
    fn root_delete_drops_all_materialized_nodes() {
        let mut tree = SparseBrickTree::centered(24, NodeState::Air).unwrap();
        tree.set(PageKey::new(0, [12, -9, 3]), NodeState::Solid(1))
            .unwrap();
        assert!(tree.node_count() <= 1 + 8 * 24);
        tree.set_root(NodeState::Air).unwrap();
        assert_eq!(tree.node_count(), 1);
    }

    #[test]
    fn canonical_hierarchy_round_trips_with_stable_hash() {
        let generator = ContentHash::of(b"generator-v1");
        let mut tree = SparseBrickTree::centered(12, NodeState::Procedural(generator)).unwrap();
        tree.set(PageKey::new(0, [-3, 7, 2]), NodeState::Solid(6))
            .unwrap();
        tree.set(
            PageKey::new(2, [4, -2, 1]),
            NodeState::Page(ContentHash::of(b"page")),
        )
        .unwrap();
        let decoded = SparseBrickTree::decode(&tree.encode()).unwrap();
        assert_eq!(decoded, tree);
        assert_eq!(decoded.content_hash(), tree.content_hash());
    }

    #[test]
    fn summary_sequences_are_canonical_and_branch_corruption_is_rejected() {
        let mut tree = SparseBrickTree::centered_with_summary(
            4,
            NodeState::Air,
            TerrainNodeSummary::uniform_air(7),
        )
        .unwrap();
        tree.set_with_summary(
            PageKey::new(0, [1, 1, 1]),
            NodeState::Solid(4),
            TerrainNodeSummary::uniform_solid(9),
            |_, inherited, _| inherited,
        )
        .unwrap();
        let encoded = tree.encode();
        let decoded = SparseBrickTree::decode(&encoded).unwrap();
        assert_eq!(decoded.max_summary_sequence(), 9);
        assert_eq!(decoded, tree);

        let mut corrupt = encoded;
        corrupt[41] ^= 1;
        assert_eq!(
            SparseBrickTree::decode(&corrupt),
            Err(HierarchyError::Codec)
        );
        assert_eq!(
            SparseBrickTree::decode(b"PTHIER01"),
            Err(HierarchyError::Codec)
        );
    }
}
