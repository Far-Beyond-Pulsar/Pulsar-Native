//! The block palette: what each block is made of (a voxel planet material
//! id) and how it looks (`look`: colour, patches, light through and given
//! off). Blocks are small voxels of procedural colour; the planet varies each
//! voxel's pigment and each region's patches, so nothing is textured.

use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::terrain::MaterialAppearance;

/// Every placeable block. The voxel planet stores a block as its material id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Grass,
    Dirt,
    Stone,
    Sand,
    Snow,
    Gravel,
    Sandstone,
    Andesite,
    OakLog,
    OakLeaves,
    Clay,
    Bricks,
    OakPlanks,
    Cobblestone,
    BirchLog,
    BirchLeaves,
    SpruceLog,
    SpruceLeaves,
    JungleLog,
    JungleLeaves,
    Cactus,
    Glowstone,
    StoneBricks,
    DarkOakPlanks,
}

impl Block {
    pub const ALL: [Block; 24] = [
        Block::Grass,
        Block::Dirt,
        Block::Stone,
        Block::Sand,
        Block::Snow,
        Block::Gravel,
        Block::Sandstone,
        Block::Andesite,
        Block::OakLog,
        Block::OakLeaves,
        Block::Clay,
        Block::Bricks,
        Block::OakPlanks,
        Block::Cobblestone,
        Block::BirchLog,
        Block::BirchLeaves,
        Block::SpruceLog,
        Block::SpruceLeaves,
        Block::JungleLog,
        Block::JungleLeaves,
        Block::Cactus,
        Block::Glowstone,
        Block::StoneBricks,
        Block::DarkOakPlanks,
    ];

    /// The blocks on the hotbar, in order.
    pub const HOTBAR: [Block; 9] = [
        Block::Cobblestone,
        Block::OakPlanks,
        Block::StoneBricks,
        Block::Bricks,
        Block::OakLog,
        Block::Glowstone,
        Block::Sand,
        Block::Dirt,
        Block::Snow,
    ];

    /// The voxel planet material id this block is stored as.
    pub fn material(self) -> u32 {
        match self {
            Block::Grass => material::GRASS,
            Block::Dirt => material::DIRT,
            Block::Stone => material::STONE,
            Block::Sand => material::SAND,
            Block::Snow => material::SNOW,
            Block::Gravel => material::GRAVEL,
            Block::Sandstone => material::SANDSTONE,
            Block::Andesite => material::DARK_STONE,
            Block::OakLog => material::WOOD,
            Block::OakLeaves => material::LEAVES,
            Block::Clay => material::CLAY,
            Block::Bricks => material::BRICK,
            Block::OakPlanks => material::PLANKS,
            Block::Cobblestone => material::COBBLE,
            // Ids past the planet's built-in materials are this game's own.
            Block::BirchLog => 22,
            Block::BirchLeaves => 23,
            Block::SpruceLog => 24,
            Block::SpruceLeaves => 25,
            Block::JungleLog => 26,
            Block::JungleLeaves => 27,
            Block::Cactus => 28,
            Block::Glowstone => 29,
            Block::StoneBricks => 30,
            Block::DarkOakPlanks => 31,
        }
    }

    /// The block a voxel material id is, if it is one.
    pub fn from_material(id: u32) -> Option<Block> {
        Block::ALL.into_iter().find(|b| b.material() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Block::Grass => "Grass",
            Block::Dirt => "Dirt",
            Block::Stone => "Stone",
            Block::Sand => "Sand",
            Block::Snow => "Snow",
            Block::Gravel => "Gravel",
            Block::Sandstone => "Sandstone",
            Block::Andesite => "Andesite",
            Block::OakLog => "Oak Log",
            Block::OakLeaves => "Oak Leaves",
            Block::Clay => "Clay",
            Block::Bricks => "Bricks",
            Block::OakPlanks => "Oak Planks",
            Block::Cobblestone => "Cobblestone",
            Block::BirchLog => "Birch Log",
            Block::BirchLeaves => "Birch Leaves",
            Block::SpruceLog => "Spruce Log",
            Block::SpruceLeaves => "Spruce Leaves",
            Block::JungleLog => "Jungle Log",
            Block::JungleLeaves => "Jungle Leaves",
            Block::Cactus => "Cactus",
            Block::Glowstone => "Glowstone",
            Block::StoneBricks => "Stone Bricks",
            Block::DarkOakPlanks => "Dark Oak Planks",
        }
    }

    /// The block's colour (sRGB) and, for natural ones, its dry, middle
    /// and lush patch colours over the land.
    fn colours(self) -> ([u8; 3], Option<[[u8; 3]; 3]>) {
        match self {
            Block::Grass => (
                [74, 104, 50],
                Some([[128, 128, 72], [74, 104, 50], [48, 84, 42]]),
            ),
            Block::Dirt => (
                [116, 86, 60],
                Some([[130, 100, 72], [116, 86, 60], [92, 68, 50]]),
            ),
            Block::Stone => (
                [128, 130, 130],
                Some([[156, 152, 142], [128, 130, 130], [98, 102, 106]]),
            ),
            Block::Sand => (
                [212, 194, 150],
                Some([[224, 208, 164], [212, 194, 150], [192, 172, 130]]),
            ),
            Block::Snow => ([234, 240, 246], None),
            Block::Gravel => (
                [124, 120, 114],
                Some([[142, 138, 130], [124, 120, 114], [98, 96, 92]]),
            ),
            Block::Sandstone => (
                [206, 176, 128],
                Some([[218, 190, 142], [206, 176, 128], [182, 150, 106]]),
            ),
            Block::Andesite => (
                [104, 108, 112],
                Some([[126, 124, 120], [104, 108, 112], [80, 84, 90]]),
            ),
            Block::OakLog => ([98, 74, 52], None),
            Block::OakLeaves => (
                [52, 86, 40],
                Some([[86, 110, 48], [52, 86, 40], [36, 68, 34]]),
            ),
            Block::Clay => ([160, 142, 130], None),
            Block::Bricks => ([152, 74, 58], None),
            Block::OakPlanks => ([172, 132, 86], None),
            Block::Cobblestone => ([118, 118, 116], None),
            Block::BirchLog => ([218, 214, 202], None),
            Block::BirchLeaves => (
                [96, 124, 54],
                Some([[130, 138, 60], [96, 124, 54], [68, 104, 46]]),
            ),
            Block::SpruceLog => ([80, 58, 42], None),
            Block::SpruceLeaves => (
                [38, 68, 46],
                Some([[60, 86, 52], [38, 68, 46], [28, 54, 40]]),
            ),
            Block::JungleLog => ([112, 90, 62], None),
            Block::JungleLeaves => (
                [48, 108, 40],
                Some([[84, 132, 46], [48, 108, 40], [32, 88, 34]]),
            ),
            Block::Cactus => ([70, 118, 58], None),
            Block::Glowstone => ([242, 204, 124], None),
            Block::StoneBricks => ([132, 132, 130], None),
            Block::DarkOakPlanks => ([88, 62, 40], None),
        }
    }

    /// The block's colour in linear light, for things that look like it
    /// (the block in hand).
    pub fn linear_colour(self) -> [f32; 3] {
        self.colours().0.map(|v| (f32::from(v) / 255.0).powf(2.2))
    }

    /// How the voxel planet draws the block.
    pub fn look(self) -> MaterialAppearance {
        let unit = |c: [u8; 3]| c.map(|v| f32::from(v) / 255.0);
        let (colour, patches) = self.colours();
        let [r, g, b] = unit(colour);
        let leaves = matches!(
            self,
            Block::OakLeaves | Block::BirchLeaves | Block::SpruceLeaves | Block::JungleLeaves
        );
        MaterialAppearance {
            colour: [r, g, b, self.roughness()],
            patches: patches.map(|p| p.map(unit)),
            lip: (self == Block::Grass).then_some(material::DIRT as u8),
            // A cell of leaves is a cluster with gaps: about two thirds of
            // the light on it gets through (lit from behind; global
            // illumination).
            transmission: if leaves { 0.65 } else { 0.0 },
            // Glowing blocks light their surroundings.
            emission: self.emission().map_or(0.0, |(_, strength)| strength),
            ..MaterialAppearance::default()
        }
    }

    /// Light the block gives off, as an emissive colour and strength.
    pub fn emission(self) -> Option<([f32; 3], f32)> {
        match self {
            Block::Glowstone => Some(([1.0, 0.78, 0.45], 3.5)),
            _ => None,
        }
    }

    /// Roughness of the block's faces.
    pub fn roughness(self) -> f32 {
        match self {
            Block::Snow => 0.7,
            Block::Glowstone => 0.35,
            Block::Sand | Block::Dirt | Block::Gravel | Block::Clay => 0.95,
            Block::Stone
            | Block::Andesite
            | Block::Cobblestone
            | Block::StoneBricks
            | Block::Bricks => 0.88,
            _ => 0.82,
        }
    }
}
