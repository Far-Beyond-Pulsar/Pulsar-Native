use crate::{ContentHash, NodeState, PageKey};
use thiserror::Error;

const HIERARCHY_MAGIC: &[u8; 8] = b"PTHIER01";

#[derive(Clone, Debug, PartialEq, Eq)]
enum TreeNode {
    Leaf(NodeState),
    Branch(Box<[TreeNode; 8]>),
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
        if root_lod == 0 || root_lod > 62 || state == NodeState::Branch {
            return Err(HierarchyError::InvalidRoot);
        }
        let half = 1_i64 << (root_lod - 1);
        Ok(Self {
            root_lod,
            root_min_page: [-half; 3],
            root: TreeNode::Leaf(state),
        })
    }

    pub fn root_state(&self) -> NodeState {
        match &self.root {
            TreeNode::Leaf(state) => state.clone(),
            TreeNode::Branch(_) => NodeState::Branch,
        }
    }

    /// Exact O(1) root replacement, including whole-planet deletion.
    pub fn set_root(&mut self, state: NodeState) -> Result<(), HierarchyError> {
        if state == NodeState::Branch {
            return Err(HierarchyError::BranchIsInternal);
        }
        self.root = TreeNode::Leaf(state);
        Ok(())
    }

    pub fn set(&mut self, key: PageKey, state: NodeState) -> Result<(), HierarchyError> {
        if state == NodeState::Branch {
            return Err(HierarchyError::BranchIsInternal);
        }
        let path = self.path_for(key)?;
        set_recursive(&mut self.root, &path, state);
        Ok(())
    }

    pub fn resolve(&self, key: PageKey) -> Result<NodeState, HierarchyError> {
        let path = self.path_for(key)?;
        let mut node = &self.root;
        for child in path {
            match node {
                TreeNode::Leaf(state) => return Ok(state.clone()),
                TreeNode::Branch(children) => node = &children[child],
            }
        }
        Ok(match node {
            TreeNode::Leaf(state) => state.clone(),
            TreeNode::Branch(_) => NodeState::Branch,
        })
    }

    pub fn node_count(&self) -> usize {
        count_nodes(&self.root)
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
        if bytes.len() < 41
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

    fn path_for(&self, key: PageKey) -> Result<Vec<usize>, HierarchyError> {
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
            path.push(x | (y << 1) | (z << 2));
        }
        Ok(path)
    }
}

fn set_recursive(node: &mut TreeNode, path: &[usize], state: NodeState) {
    if path.is_empty() {
        *node = TreeNode::Leaf(state);
        return;
    }
    if let TreeNode::Leaf(previous) = node {
        let child = TreeNode::Leaf(previous.clone());
        *node = TreeNode::Branch(Box::new(std::array::from_fn(|_| child.clone())));
    }
    let TreeNode::Branch(children) = node else {
        unreachable!();
    };
    set_recursive(&mut children[path[0]], &path[1..], state);

    let first = match &children[0] {
        TreeNode::Leaf(first) => first.clone(),
        TreeNode::Branch(_) => return,
    };
    if children
        .iter()
        .all(|child| matches!(child, TreeNode::Leaf(value) if *value == first))
    {
        *node = TreeNode::Leaf(first);
    }
}

fn count_nodes(node: &TreeNode) -> usize {
    match node {
        TreeNode::Leaf(_) => 1,
        TreeNode::Branch(children) => 1 + children.iter().map(count_nodes).sum::<usize>(),
    }
}

fn encode_node(node: &TreeNode, output: &mut Vec<u8>) {
    match node {
        TreeNode::Leaf(NodeState::Air) => output.push(0),
        TreeNode::Leaf(NodeState::Solid(material)) => {
            output.push(1);
            output.push(*material);
        }
        TreeNode::Leaf(NodeState::Procedural(hash)) => {
            output.push(2);
            output.extend_from_slice(&hash.0);
        }
        TreeNode::Branch(children) => {
            output.push(3);
            for child in children.iter() {
                encode_node(child, output);
            }
        }
        TreeNode::Leaf(NodeState::Page(hash)) => {
            output.push(4);
            output.extend_from_slice(&hash.0);
        }
        TreeNode::Leaf(NodeState::Branch) => unreachable!("Branch is stored structurally"),
    }
}

fn decode_node(
    bytes: &[u8],
    cursor: &mut usize,
    depth: u8,
    root_lod: u8,
) -> Result<TreeNode, HierarchyError> {
    let tag = *bytes.get(*cursor).ok_or(HierarchyError::Codec)?;
    *cursor += 1;
    match tag {
        0 => Ok(TreeNode::Leaf(NodeState::Air)),
        1 => {
            let material = *bytes.get(*cursor).ok_or(HierarchyError::Codec)?;
            *cursor += 1;
            Ok(TreeNode::Leaf(NodeState::Solid(material)))
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
            Ok(TreeNode::Leaf(if tag == 2 {
                NodeState::Procedural(hash)
            } else {
                NodeState::Page(hash)
            }))
        }
        3 if depth < root_lod => {
            let mut children = Vec::with_capacity(8);
            for _ in 0..8 {
                children.push(decode_node(bytes, cursor, depth + 1, root_lod)?);
            }
            Ok(TreeNode::Branch(Box::new(
                children.try_into().map_err(|_| HierarchyError::Codec)?,
            )))
        }
        _ => Err(HierarchyError::Codec),
    }
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
                    tree.set(
                        PageKey::new(0, [-2 + x, y, z]),
                        NodeState::Solid(4),
                    )
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
        tree.set(PageKey::new(0, [12, -9, 3]), NodeState::Solid(1)).unwrap();
        assert!(tree.node_count() <= 1 + 8 * 24);
        tree.set_root(NodeState::Air).unwrap();
        assert_eq!(tree.node_count(), 1);
    }

    #[test]
    fn canonical_hierarchy_round_trips_with_stable_hash() {
        let generator = ContentHash::of(b"generator-v1");
        let mut tree = SparseBrickTree::centered(12, NodeState::Procedural(generator)).unwrap();
        tree.set(PageKey::new(0, [-3, 7, 2]), NodeState::Solid(6)).unwrap();
        tree.set(
            PageKey::new(2, [4, -2, 1]),
            NodeState::Page(ContentHash::of(b"page")),
        )
        .unwrap();
        let decoded = SparseBrickTree::decode(&tree.encode()).unwrap();
        assert_eq!(decoded, tree);
        assert_eq!(decoded.content_hash(), tree.content_hash());
    }
}
