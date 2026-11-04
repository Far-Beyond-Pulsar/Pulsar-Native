//! Museum dimensions, read from `assets/vg_stress/museum_dims.json`, the
//! file the kit shell pieces were sized from. All values are
//! metres in engine axes (+Y up, +Z toward the entrance).

use serde::Deserialize;

const DIMS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/vg_stress/museum_dims.json"
));

#[derive(Clone, Debug, Deserialize)]
pub struct Pool {
    pub half_w: f32,
    pub z0: f32,
    pub z1: f32,
    pub rim: f32,
    pub depth: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Plaza {
    pub half_w: f32,
    pub z0: f32,
    pub z1: f32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Dims {
    pub spacing: f32,
    pub front_cols: usize,
    pub side_cols: usize,
    pub stylobate_y: f32,
    pub step_h: f32,
    pub step_tread: f32,
    pub stylobate_half: [f32; 2],
    pub doric_height: f32,
    pub arch_h: f32,
    pub frieze_h: f32,
    pub cornice_h: f32,
    pub cornice_out: f32,
    pub entab_end_extension: f32,
    pub cella_half_w: f32,
    pub cella_wall_t: f32,
    pub cella_z: f32,
    pub door_w: f32,
    pub door_h: f32,
    pub ionic_height: f32,
    pub ionic_x: f32,
    pub ionic_z: Vec<f32>,
    pub aisle_ceiling_y: f32,
    pub peristyle_ceiling_y: f32,
    pub pediment_h: f32,
    pub pool: Pool,
    pub plaza: Plaza,
}

impl Dims {
    /// The committed dimensions file.
    pub fn load() -> Self {
        serde_json::from_str(DIMS_JSON).expect("museum_dims.json parses")
    }

    /// Half extent of the column rectangle across (x) and along (z).
    pub fn column_half(&self) -> (f32, f32) {
        (
            self.spacing * (self.front_cols as f32 - 1.0) / 2.0,
            self.spacing * (self.side_cols as f32 - 1.0) / 2.0,
        )
    }

    /// World height of the top of the Doric columns (underside of the architrave).
    pub fn doric_top(&self) -> f32 {
        self.stylobate_y + self.doric_height
    }

    /// World height of the top of the cornice (the eave line).
    pub fn eave_y(&self) -> f32 {
        self.doric_top() + self.arch_h + self.frieze_h + self.cornice_h
    }

    /// Height of the finished marble floor surface.
    pub fn floor_y(&self) -> f32 {
        self.stylobate_y + 0.03
    }
}
