//! The terrain-stack interpreter of `helio.terrain` ([`crate::layers`]).
//!
//! A stack compiles to one octave table sorted from coarsest to finest,
//! each octave tagged with its layer (`kind | layer << 8`), plus a table of
//! layers. [`height_parts`] runs the octaves into per-layer accumulators,
//! then composes the layers in stack order with their masks; every world
//! (planet, moon, plane) runs this same code, and `landform.wgsl` mirrors
//! it, so changing layers never recompiles shaders.
//!
//! Every operation is wrapping two's-complement integer arithmetic, so CPU
//! and GPU agree to the bit. Additive octaves finer than a column footprint
//! are omitted: coarse levels are band-limited point samples of the same
//! field rather than an independent smooth replacement.
use crate::grid::Grid;
use crate::noise::{
    fade, hash3, lerp, mul16, mul_fine, mul_shr, mul_shr_signed, noise, noise_fine,
    noise_fine_grad, scale, sin_turns, unit_q30, FINE_ONE, ONE, Q30,
};
use crate::planet_maps::{self, MapView, PlanetMaps, MAP_HEADER};
use crate::terrain::{Column, TerrainAppearance, TerrainField, TerrainProgram, HEIGHT_ONE};
use crate::volumes::VolumeLayer;
use bytemuck::{Pod, Zeroable};
use glam::IVec3;
use std::borrow::Cow;
use std::sync::Arc;

/// Program key: one WGSL source for every stack; generation compiles the
/// ridge-envelope display variant of it.
pub(crate) const DISPLAY_PROGRAM: &str = "helio.terrain/1-stack-ridge-envelope";

/// Quantile of [`noise`] (in units of [`ONE`]), from 400k samples: symmetric,
/// standard deviation 0.27, slightly lighter tails than a Gaussian
/// (`tests::noise_distribution_quantiles`).
pub(crate) fn noise_quantile(p: f64) -> f64 {
    const TABLE: [(f64, f64); 6] = [
        (0.5, 0.0),
        (0.75, 0.1937),
        (0.9, 0.3533),
        (0.95, 0.446),
        (0.99, 0.5851),
        (1.0, 0.75),
    ];
    let (sign, p) = if p < 0.5 { (-1.0, 1.0 - p) } else { (1.0, p) };
    let mut value = TABLE[5].1;
    for w in TABLE.windows(2) {
        if p <= w[1].0 {
            value = w[0].1 + (w[1].1 - w[0].1) * (p - w[0].0) / (w[1].0 - w[0].0);
            break;
        }
    }
    sign * value
}

/// Octave table size, warp included.
pub const OCTAVES: usize = 64;
/// Leading domain-warp octaves (two per axis, zero without a Warp layer).
pub const WARP_OCTAVES: usize = 6;
/// Layers of a stack.
pub const LAYERS: usize = 8;

/// Octave kinds (low byte of [`Octave::kind`]; the layer index is above).
pub const CONTINENT: u32 = 0;
/// Mountain region mask.
pub const REGION: u32 = 1;
pub const RIDGE: u32 = 2;
/// Warped fBm (fine noise).
pub const HILLS: u32 = 3;
/// Domain warp, one kind per axis (4, 5, 6).
pub const WARP: u32 = 4;
/// Unwarped metre-scale fBm (16-bit noise).
pub const ROUGHNESS: u32 = 7;
/// Erosion gullies.
pub const EROSION: u32 = 8;
/// Crater lattice; its density (Q16) is the seed's low 16 bits.
pub const CRATER: u32 = 9;
/// Basin mask (fine noise, unwarped).
pub const BASIN: u32 = 10;

/// One octave: lattice shift, amplitude (height units; Q16 weight for the
/// masks) and seed, and `kind | layer << 8`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct Octave {
    pub shift: u32,
    pub amplitude: i32,
    pub seed: u32,
    pub kind: u32,
}

impl Octave {
    #[inline]
    pub fn class(&self) -> u32 {
        self.kind & 0xff
    }
    #[inline]
    pub fn layer(&self) -> usize {
        ((self.kind >> 8) & (LAYERS as u32 - 1)) as usize
    }
}

/// One layer of the compiled stack: its kind, mask and two parameters.
///
/// | kind | `a` | `b` |
/// |---|---|---|
/// | continents | ocean floor (height units, negative) | lowland height |
/// | mountains | region mask bias (Q16) | |
/// | erosion | saturation slope (height units per gradient span) | |
/// | craters | rim over depth (Q16) | share of fresh craters (Q16) |
/// | basins | depth (height units) | mask threshold (Q24) |
/// | plateau | height (height units, whole layers) | |
/// | cliffs (riser share in 1/1024 in kind bits 16..) | step (height units, whole layers) | mask threshold (Q24) |
/// | rivers | | |
///
/// Rivers carve what lies below them in the stack along the planet maps'
/// river segments (`planet_maps::carve`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct StackLayer {
    pub kind: u32,
    /// 0 everywhere, 1 land, 2 above deep sea.
    pub mask: u32,
    pub a: i32,
    pub b: i32,
}

impl StackLayer {
    pub const WARP: u32 = 1;
    pub const CONTINENTS: u32 = 2;
    pub const MOUNTAINS: u32 = 3;
    pub const HILLS: u32 = 4;
    pub const ROUGHNESS: u32 = 5;
    pub const EROSION: u32 = 6;
    pub const CRATERS: u32 = 7;
    pub const BASINS: u32 = 8;
    pub const PLATEAU: u32 = 9;
    pub const CLIFFS: u32 = 10;
    pub const RIVERS: u32 = 11;

    /// The kind without its parameter bits.
    pub fn base_kind(&self) -> u32 {
        self.kind & 0xffff
    }
}

/// Terraces of `step` height units: flat treads, rising over the last
/// `riser_q10` / 1024 of each step. 32-bit integer arithmetic, as
/// `landform_terrace` in WGSL (`step` at most 2^20, `riser_q10` at most 1024).
pub fn terrace(h: i32, step: i32, riser_q10: i32) -> i32 {
    let q = h.div_euclid(step);
    let f = h - q * step;
    let riser = ((step * riser_q10) >> 10).max(1);
    let t = (((f - (step - riser)).max(0)) << 10) / riser;
    q * step + ((t.min(1024) * step) >> 10)
}

/// Material styles (`LandformConstants::style[0]`).
pub const STYLE_EARTHLIKE: i32 = 0;
pub const STYLE_LUNAR: i32 = 1;
pub const STYLE_LAYERED: i32 = 2;
/// An ordered list of material rules (`LandformConstants::rules`).
pub const STYLE_RULES: i32 = 3;
/// Material rules a stack may have.
pub const RULES: usize = 16;

/// One packed material rule (`MaterialRule` in WGSL, from
/// [`crate::layers::MaterialRule`]): every condition must hold.
/// Ranges are inclusive.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct PackedRule {
    /// material id, speck share (Q8, 0: every cell), patch lattice shift (0:
    /// no patches), patch threshold (Q16).
    pub head: [i32; 4],
    /// column height range (mm), slope range (eighths of a cell per cell).
    pub height_slope: [i32; 4],
    /// depth range (cells below the column top), moisture range (Q16).
    pub depth_moisture: [i32; 4],
    /// erosion range (the signed surface byte), strata band period (mm, 0:
    /// none) and parity.
    pub surface_bands: [i32; 4],
    /// temperature range (1/128 °C at the cell), river beds (0 any, 1 only
    /// on them, 2 never), pad.
    pub climate: [i32; 4],
}

/// `TerrainConstants` of `landform.wgsl` (uniform layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct LandformConstants {
    /// octave count (warp slots included), layer thickness (mm), soil
    /// depth (cells), seed.
    pub header: [i32; 4],
    /// pad, continents layer + 1 (0: none, all land), pad, low-basin height
    /// (mm).
    pub levels: [i32; 4],
    /// ridge display layer + 1 (0: none), steep slope (cells/cell), ridge
    /// display flag, vertical axis (0 radial, 1 the plane's +Y).
    pub shape: [i32; 4],
    /// material style, then the Layered surface, soil and rock ids.
    pub style: [i32; 4],
    /// layer count, sum of the erosion amplitudes (height units), domain
    /// radius (0 on planes), warp octaves (0 or 6).
    pub stack: [i32; 4],
    /// Rules style: rule count, fallback material; the coarsest erosion
    /// octave's shift (-1: none: erosion is resolved up to its level less
    /// `RESOLVED_SHIFT`), pad.
    pub materials: [i32; 4],
    /// The planet maps' header (`planet_maps::MAP_HEADER`): climate and
    /// river lookups, uniform climate without maps.
    pub maps: [[i32; 4]; MAP_HEADER],
    pub layers: [StackLayer; LAYERS],
    pub octaves: [Octave; OCTAVES],
    pub rules: [PackedRule; RULES],
}

/// Crater radius of the largest crater of a cell (Q19 of the lattice cell,
/// 0.3): with the ejecta at twice the radius, the 3x3x3 cells around a
/// point hold every crater reaching it.
pub const CRATER_RADIUS_Q19: u32 = 157_286;
/// Squared ejecta reach in crater radii (Q24).
const REACH: i32 = 4 << 24;

impl LandformConstants {
    fn stack_layers(&self) -> &[StackLayer] {
        &self.layers[..(self.stack[0].max(0) as usize).min(LAYERS)]
    }

    fn table(&self) -> &[Octave] {
        &self.octaves[WARP_OCTAVES..(self.header[0].max(WARP_OCTAVES as i32) as usize).min(OCTAVES)]
    }

    /// The continents layer, if the stack has one.
    fn continents(&self) -> Option<usize> {
        (self.levels[1] > 0).then(|| (self.levels[1] - 1) as usize & (LAYERS - 1))
    }

    /// Largest change (height units) each layer's own term can make.
    fn magnitudes(&self) -> [f64; LAYERS] {
        let mut m = [0.0; LAYERS];
        for o in self.table() {
            let a = f64::from(o.amplitude).abs();
            m[o.layer()] += match o.class() {
                CONTINENT | REGION | BASIN => 0.0,
                // Up to two overlapping craters per octave reach a point.
                CRATER => 2.0 * a * (1.0 + f64::from(self.layers[o.layer()].a) / 65_536.0),
                _ => a,
            };
        }
        for (l, layer) in self.stack_layers().iter().enumerate() {
            m[l] += match layer.kind {
                StackLayer::CONTINENTS => f64::from(layer.a).abs() + f64::from(layer.b).abs(),
                // Rivers cut at most this far under the terrain's lowest.
                StackLayer::BASINS | StackLayer::PLATEAU | StackLayer::RIVERS => {
                    f64::from(layer.a).abs()
                }
                // A terrace moves a height by less than a step.
                _ if layer.base_kind() == StackLayer::CLIFFS => f64::from(layer.a).abs(),
                _ => 0.0,
            };
        }
        m
    }

    /// Per layer, the largest change the layer makes as composed: its own
    /// term, and for basins also flattening the layers before them by half.
    fn effects(&self) -> [f64; LAYERS] {
        let m = self.magnitudes();
        let mut before = 0.0;
        let mut e = m;
        for (l, layer) in self.stack_layers().iter().enumerate() {
            if layer.kind == StackLayer::BASINS {
                e[l] += before / 2.0;
            }
            before += e[l];
        }
        e
    }

    /// Conservative per-level surface excess, in level cells (see
    /// [`TerrainField::bound_margins`]).
    ///
    /// A finer level adds octaves that this level omits (each bounded by its
    /// amplitude) and the resolved field varies inside the cell by at most its
    /// Lipschitz constant times the half diagonal. The noise gradient bound
    /// `G` is 1.5x the measured maximum of the fixed-point gradient noise
    /// (5.3 per lattice spacing); `check_field` samples the result.
    pub fn bound_margins(&self, grid: &Grid) -> [i32; 24] {
        const G: f64 = 8.0;
        let magnitude = self.magnitudes();
        let effect = self.effects();
        let layers = self.stack_layers();
        let masked = |mask: u32| {
            layers
                .iter()
                .enumerate()
                .filter(|(_, l)| l.mask == mask)
                .map(|(l, _)| effect[l])
                .sum::<f64>()
        };
        // The continents scale the base, the land mask (x3) and the deep-sea
        // fade (x2) of the layers they mask.
        let shape = self.continents().map_or(0.0, |l| {
            f64::from(layers[l].a).abs() + 2.0 * f64::from(layers[l].b).abs()
        });
        let continent = shape + 3.0 * masked(1) + 2.0 * masked(2);
        // Domain warp Lipschitz constant (dimensionless).
        let warp = self.octaves[..self.stack[3] as usize]
            .iter()
            .map(|o| f64::from(o.amplitude.abs()) * G / 2f64.powi(o.shift as i32))
            .sum::<f64>();
        let ratio = f64::from(grid.reference_cells()) / f64::from(grid.cells());
        let mut out = [0i32; 24];
        for level in 0..24u32 {
            let effective = level + grid.level_offset();
            let mut dropped = 0.0;
            let mut lipschitz = 0.0;
            let mut unwarped = 0.0;
            for o in self.table() {
                let a = f64::from(o.amplitude.abs());
                let per = 1.0 / 2f64.powi(o.shift as i32);
                let relief = match o.class() {
                    CRATER => 2.0 * a * (1.0 + f64::from(self.layers[o.layer()].a) / 65_536.0),
                    _ => a,
                };
                match o.class() {
                    CONTINENT => lipschitz += continent * G * per,
                    REGION => lipschitz += 3.0 * magnitude[o.layer()] * G * per,
                    BASIN => unwarped += 4.0 * effect[o.layer()] * G * per,
                    _ if !resolved(o, effective) => dropped += relief,
                    RIDGE => lipschitz += 2.0 * a * G * per,
                    HILLS => lipschitz += a * G * per,
                    // Gullies: the phase turns 2 pi STRIPES per lattice
                    // spacing, plus the turning of their direction.
                    EROSION => unwarped += a * 19.0 * per,
                    // Bowl slope 2 (depth + rim) per radius, radius >= 0.165 cells.
                    CRATER => unwarped += relief * 13.0 * per,
                    _ => unwarped += a * G * per,
                }
            }
            // Half diagonal of a level cell in domain units (sphere cells
            // are at most 1.05 times a face-centre cell).
            let cell = crate::grid::REFERENCE_VOXEL / crate::grid::DOMAIN_UNIT;
            let half_diagonal =
                2f64.powi(level as i32) * cell * ratio * std::f64::consts::SQRT_2 * 0.5 * 1.05;
            // Terraces are not Lipschitz: a riser can rise a whole step
            // inside a cell, and heights a step apart terrace up to two
            // steps apart.
            let terraces: f64 = layers
                .iter()
                .filter(|l| l.base_kind() == StackLayer::CLIFFS)
                .map(|l| 2.0 * f64::from(l.a))
                .sum();
            // River valleys: their walls, beds and channels slope at most
            // `b / 256` height units per domain unit (unwarped).
            let valleys: f64 = layers
                .iter()
                .filter(|l| l.kind == StackLayer::RIVERS)
                .map(|l| f64::from(l.b) / 256.0)
                .sum();
            let excess_mm = dropped
                + terraces
                + (lipschitz * (1.0 + warp) + unwarped + valleys) * half_diagonal;
            let cell_mm = f64::from(self.header[1]) * 2f64.powi(level as i32);
            out[level as usize] =
                ((excess_mm / cell_mm).ceil() as i64 + 2).clamp(2, 1 << 20) as i32;
        }
        out
    }

    /// Conservative lowest and highest surface height (height units).
    pub fn height_range(&self) -> (i32, i32) {
        let layers = self.stack_layers();
        let effect = self.effects();
        let plateau: f64 = layers
            .iter()
            .filter(|l| l.kind == StackLayer::PLATEAU)
            .map(|l| f64::from(l.a))
            .sum();
        let basins = layers.iter().any(|l| l.kind == StackLayer::BASINS);
        let mut spread = 0.0;
        for (l, layer) in layers.iter().enumerate() {
            // Basins may flatten a plateau: then it is spread, not an offset.
            if layer.kind != StackLayer::PLATEAU || basins {
                spread += effect[l];
            }
        }
        let centre = if basins { 0.0 } else { plateau };
        let pad = if spread > 0.0 {
            10.0 * f64::from(HEIGHT_ONE)
        } else {
            0.0
        };
        let clamp = |v: f64| v.clamp(f64::from(i32::MIN / 2), f64::from(i32::MAX / 2)) as i32;
        (
            clamp((centre - spread - pad).floor()),
            clamp((centre + spread + pad).ceil()),
        )
    }
}

/// Smallest lattice shift resolved at `level`, above the level: a lattice
/// of `2^(level + 5)` domain units spans four level cells.
pub const RESOLVED_SHIFT: u32 = 5;

/// Additive detail finer than about four level cells is omitted at `level`
/// (`level` counts reference cells); masks are always evaluated.
#[inline]
fn resolved(o: &Octave, level: u32) -> bool {
    matches!(o.class(), CONTINENT | REGION | BASIN) || o.shift >= level + RESOLVED_SHIFT
}

/// Largest relief (height units) of the octaves `level` omits: what finer
/// levels may add to its heights (the planet maps' bound on the terrain a
/// map texel does not resolve).
pub fn unresolved_relief(k: &LandformConstants, level: u32) -> f64 {
    k.table()
        .iter()
        .filter(|o| !resolved(o, level))
        .map(|o| {
            let a = f64::from(o.amplitude).abs();
            match o.class() {
                CRATER => 2.0 * a * (1.0 + f64::from(k.layers[o.layer()].a) / 65_536.0),
                _ => a,
            }
        })
        .sum()
}

/// Gradients are in height units per `2^GRAD_SHIFT` domain units (3.3 km):
/// gentle slopes keep about 20 bits, which gully directions need.
pub const GRAD_SHIFT: u32 = 18;
/// Gully stripes per erosion lattice spacing.
const STRIPES: i32 = 2;

/// A lattice-spacing derivative per gradient span.
#[inline]
fn per_span(d: i32, shift: u32) -> i32 {
    if shift >= GRAD_SHIFT {
        d >> (shift - GRAD_SHIFT)
    } else {
        d << (GRAD_SHIFT - shift)
    }
}

#[inline]
fn per_span3(d: IVec3, shift: u32) -> IVec3 {
    IVec3::new(
        per_span(d.x, shift),
        per_span(d.y, shift),
        per_span(d.z, shift),
    )
}

#[inline]
fn mul_fine3(v: IVec3, b: i32) -> IVec3 {
    IVec3::new(mul_fine(v.x, b), mul_fine(v.y, b), mul_fine(v.z, b))
}

/// Q24 weight of a clamp's derivative inside `(lo, hi)`, fading to zero over
/// `FINE_ONE / 8` at both edges. Gullies are steered by a continuous
/// gradient: a clamp's derivative switches on and off at its edges, and
/// the direction jump would shift the gully phase into a cliff.
#[inline]
fn soft_inside(x: i32, lo: i32, hi: i32) -> i32 {
    ((x - lo) * 8)
        .clamp(0, FINE_ONE)
        .min(((hi - x) * 8).clamp(0, FINE_ONE))
}

/// One erosion octave at `p`: gully stripes along the downhill direction of
/// `g` (height units per span w.r.t. `p`), blended over the octave's 3D
/// lattice with a random phase per corner, and their gradient (which steers
/// the finer octaves). `up` is the Q30 unit vertical.
///
/// The phase turns up to `2 sqrt(3) STRIPES` times across a lattice cell, so
/// direction and offsets keep full precision (Q30 unit vectors, exact 64-bit
/// products): a 1e-5 direction error would already shift a 40 m gully by
/// centimetres between neighbouring columns.
/// `saturation` is the slope (height units per span) of full depth.
fn erosion_octave(saturation: i32, o: &Octave, p: IVec3, g: IVec3, up: IVec3) -> (i32, IVec3) {
    // Across the slope, horizontal: t = up x g, |t| the horizontal slope.
    let t = IVec3::new(
        mul_shr_signed(up.y, g.z, 30) - mul_shr_signed(up.z, g.y, 30),
        mul_shr_signed(up.z, g.x, 30) - mul_shr_signed(up.x, g.z, 30),
        mul_shr_signed(up.x, g.y, 30) - mul_shr_signed(up.y, g.x, 30),
    );
    let tn = unit_q30(t);
    if tn == IVec3::ZERO {
        return (0, IVec3::ZERO);
    }
    let slope = (mul_shr_signed(t.x, tn.x, 30)
        + mul_shr_signed(t.y, tn.y, 30)
        + mul_shr_signed(t.z, tn.z, 30)) as u32;
    let saturation = saturation.max(1) as u32;
    let strength = (((slope.min(saturation) >> 5) << 16) / (saturation >> 5).max(1)) as i32;
    let s = o.shift;
    let c = IVec3::new(p.x >> s, p.y >> s, p.z >> s);
    let mask = (1i32 << s) - 1;
    let f = [p.x, p.y, p.z].map(|v| {
        if s >= 16 {
            (v & mask) >> (s - 16)
        } else {
            (v & mask) << (16 - s)
        }
    });
    let w = f.map(fade);
    // Offset along tn, in Q16 lattice spacings: from the cell's first corner,
    // less one step of tn (Q16) for each axis a corner is past it.
    let along0 = mul_shr_signed(f[0], tn.x, 30)
        + mul_shr_signed(f[1], tn.y, 30)
        + mul_shr_signed(f[2], tn.z, 30);
    let step = IVec3::new(tn.x >> 14, tn.y >> 14, tn.z >> 14);
    let mut cos = [0i32; 8];
    let mut sin = [0i32; 8];
    for index in 0..8usize {
        let k = IVec3::new(
            (index & 1) as i32,
            ((index >> 1) & 1) as i32,
            (index >> 2) as i32,
        );
        let corner = c + k;
        let along = along0 - (k * step).element_sum();
        let phase = along * STRIPES + (hash3(corner.x, corner.y, corner.z, o.seed) & 0xffff) as i32;
        // Gully profile pi/2 |cos(phase / 2)| - 1: V-shaped valleys between
        // rounded ridges, zero mean (levels that omit it keep their height)
        // and within the amplitude.
        cos[index] = mul16(sin_turns((phase >> 1) + 16_384).abs(), 102_944) - ONE;
        sin[index] = sin_turns(phase);
    }
    let tri = |v: [i32; 8]| {
        let y0 = lerp(lerp(v[0], v[1], w[0]), lerp(v[2], v[3], w[0]), w[1]);
        let y1 = lerp(lerp(v[4], v[5], w[0]), lerp(v[6], v[7], w[0]), w[1]);
        lerp(y0, y1, w[2])
    };
    let value = scale(mul16(tri(cos), strength), o.amplitude);
    // The steering derivative is the smooth cos(phase) one, so finer octaves
    // see no direction flip at valley floors: -A s sin(phase) 2 pi STRIPES /
    // lattice along tn (2 pi STRIPES = 3217 / 256).
    let m = scale(mul16(tri(sin), strength), o.amplitude);
    let magnitude = per_span(-((m * 3_217) >> 8), s);
    (
        value,
        IVec3::new(
            mul_shr_signed(magnitude, tn.x, 30),
            mul_shr_signed(magnitude, tn.y, 30),
            mul_shr_signed(magnitude, tn.z, 30),
        ),
    )
}

/// Ocean floor to lowland base of the continents at `c` (Q24): shelf then
/// deep ocean below the coast, lowlands rising inland.
#[inline]
fn continent_base(c: i32, floor: i32, lowland: i32) -> i32 {
    if c < 0 {
        let t = (-c).min(FINE_ONE);
        mul_fine(floor, t).wrapping_add(mul_fine(lowland / 8, FINE_ONE - t))
    } else {
        mul_fine(lowland, (c * 2).min(FINE_ONE))
    }
}

/// Land weight (rises from the coast) and deep-sea fade at continent value
/// `c`; without continents `c` is `FINE_ONE`, all land.
#[inline]
fn land_masks(c: i32) -> (i32, i32) {
    (
        (c * 3).clamp(0, FINE_ONE),
        (FINE_ONE + c * 2).clamp(FINE_ONE / 8, FINE_ONE),
    )
}

#[inline]
fn masked(mask: u32, x: i32, land: i32, wet: i32) -> i32 {
    match mask {
        1 => mul_fine(x, land),
        2 => mul_fine(x, wet),
        _ => x,
    }
}

/// Mountain region weight of a mountain layer from its mask noise.
#[inline]
fn region_weight(region: i32, bias: i32) -> i32 {
    ((region - (bias << 8)) * 3).clamp(0, FINE_ONE)
}

/// Per-layer accumulators of [`height_parts`] and their gradients.
struct Accumulators {
    /// continents: Q24 value; mountains: ridged sum; basins: Q24 mask noise;
    /// other layers: height.
    value: [i32; LAYERS],
    /// mountains: Q24 region noise.
    region: [i32; LAYERS],
    /// mountains: ridge weight of the next octave.
    weight: [i32; LAYERS],
    dvalue: [IVec3; LAYERS],
    dregion: [IVec3; LAYERS],
    dweight: [IVec3; LAYERS],
}

/// Steering gradient (height units per span, w.r.t. the warped point) of
/// the continents and mountains resolved so far, masked as composed, with
/// every kink softened ([`soft_inside`]).
fn steering(k: &LandformConstants, s: &Accumulators) -> IVec3 {
    let (c, dc) = k
        .continents()
        .map_or((FINE_ONE, IVec3::ZERO), |l| (s.value[l], s.dvalue[l]));
    let (land, wet) = land_masks(c);
    let dland = mul_fine3(dc * 3, soft_inside(c * 3, 0, FINE_ONE));
    let dwet = mul_fine3(
        dc * 2,
        soft_inside(FINE_ONE + c * 2, FINE_ONE / 8, FINE_ONE),
    );
    let mut g = IVec3::ZERO;
    for (l, layer) in k.stack_layers().iter().enumerate() {
        let (x, dx) = match layer.kind {
            StackLayer::CONTINENTS => {
                let d = if c < 0 {
                    mul_fine3(
                        mul_fine3(-dc, soft_inside(-c, 0, FINE_ONE)),
                        layer.a - layer.b / 8,
                    )
                } else {
                    mul_fine3(mul_fine3(dc * 2, soft_inside(c * 2, 0, FINE_ONE)), layer.b)
                };
                (continent_base(c, layer.a, layer.b), d)
            }
            StackLayer::MOUNTAINS => {
                let m = (s.region[l] - (layer.a << 8)) * 3;
                let region = m.clamp(0, FINE_ONE);
                let dregion = mul_fine3(s.dregion[l] * 3, soft_inside(m, 0, FINE_ONE));
                (
                    mul_fine(s.value[l], region),
                    mul_fine3(s.dvalue[l], region) + mul_fine3(dregion, s.value[l]),
                )
            }
            _ => continue,
        };
        g += match layer.mask {
            1 => mul_fine3(dx, land) + mul_fine3(dland, x),
            2 => mul_fine3(dx, wet) + mul_fine3(dwet, x),
            _ => dx,
        };
    }
    g
}

/// `2^60 / d` for `d` in `[2^29, 2^30)`: Newton from a linear guess, four
/// fixed steps on exact 64-bit products.
#[inline]
pub(crate) fn recip_q30(d: u32) -> u32 {
    let mut y = 3_031_741_621u32 - mul_shr(d, 2_021_161_080, 30);
    for _ in 0..4 {
        y = mul_shr(y, 2 * Q30 - mul_shr(d, y, 30), 30);
    }
    y
}

/// `r2 / rad2` in Q24 (`r2 < 4 rad2`, `rad2 > 0`).
#[inline]
fn squared_ratio(r2: u32, rad2: u32) -> u32 {
    let bits = 32 - rad2.leading_zeros();
    mul_shr(r2, recip_q30(rad2 << (30 - bits)), 6 + bits)
}

/// Polynomial smooth minimum of `a` and `b` over a width `k` (mm).
#[inline]
pub(crate) fn smooth_min(a: i32, b: i32, k: i32) -> i32 {
    let m = a.min(b);
    let gap = (a - b).abs();
    if k <= 0 || gap >= k {
        return m;
    }
    // (k - gap) / k in Q16, exactly (two division steps in 32 bits).
    let n = (k - gap) << 8;
    let q = ((n / k) << 8) + ((n % k) << 8) / k;
    m - scale(mul16(q, q), k) / 4
}

/// One crater octave at `p` (vertical `up`, Q30): height (mm) and the
/// freshest ejecta reaching `p` (0..255).
///
/// An octave has one candidate crater per cell of a 3D lattice, at a hashed
/// point; it exists with the octave's density when that point lies within
/// half a cell of the surface (sphere or plane), so craters are seamless
/// across cube edges. Each has a parabolic bowl, a rim and an ejecta
/// blanket out to twice its radius.
fn crater_octave(
    k: &LandformConstants,
    layer: &StackLayer,
    o: &Octave,
    p: IVec3,
    up: IVec3,
) -> (i32, u32) {
    let s = o.shift;
    let density = o.seed & 0xffff;
    let radius_domain = k.stack[2] as u32;
    let c0 = IVec3::new(p.x >> s, p.y >> s, p.z >> s);
    let (mut height, mut ejecta) = (0i32, 0u32);
    for index in 0..27 {
        let cell = c0 + IVec3::new(index % 3 - 1, (index / 3) % 3 - 1, index / 9 - 1);
        let a = hash3(cell.x, cell.y, cell.z, o.seed);
        if a & 0xffff >= density {
            continue;
        }
        let b = hash3(cell.x, cell.y, cell.z, o.seed ^ 0x6C8E_9CF5);
        let jitter = IVec3::new(
            (b & 1023) as i32,
            ((b >> 10) & 1023) as i32,
            ((b >> 20) & 1023) as i32,
        );
        let centre = IVec3::new(
            (cell.x << s) + (jitter.x << (s - 10)),
            (cell.y << s) + (jitter.y << (s - 10)),
            (cell.z << s) + (jitter.z << (s - 10)),
        );
        // Only centres within half a cell of the surface.
        if k.shape[3] != 0 {
            if centre.y.abs() >= 1 << (s - 1) {
                continue;
            }
        } else {
            let len2 = mul_shr(centre.x.unsigned_abs(), centre.x.unsigned_abs(), 30)
                + mul_shr(centre.y.unsigned_abs(), centre.y.unsigned_abs(), 30)
                + mul_shr(centre.z.unsigned_abs(), centre.z.unsigned_abs(), 30);
            let r2 = mul_shr(radius_domain, radius_domain, 30);
            if (len2 as i32 - r2 as i32).unsigned_abs() >= radius_domain >> (30 - s) {
                continue;
            }
        }
        // Squared horizontal distance in Q28 lattice cells; nothing beyond
        // a cell reaches `p`. Offsets are Q(19 + e) cells: Q19 for fine
        // lattices, whole domain units (e = s - 19) for coarse ones, whose
        // squares are exact 64-bit products. Rounding the offsets of a large
        // crater to Q19 would move its steep walls by centimetres between
        // neighbouring columns.
        let e = s.saturating_sub(19);
        let d = centre - p;
        let d = if s >= 19 {
            d
        } else {
            IVec3::new(d.x << (19 - s), d.y << (19 - s), d.z << (19 - s))
        };
        if d.abs().max_element() >= 1 << (19 + e) {
            continue;
        }
        // The centre may lie half a cell off the surface: the vertical part
        // cancels, so it keeps 10 more bits (Q29).
        let along = mul_shr_signed(d.x, up.x, 20 + e)
            + mul_shr_signed(d.y, up.y, 20 + e)
            + mul_shr_signed(d.z, up.z, 20 + e);
        let square = |v: i32| mul_shr(v.unsigned_abs(), v.unsigned_abs(), 10 + 2 * e);
        let along2 = mul_shr(along.unsigned_abs(), along.unsigned_abs(), 30);
        let r2 = (square(d.x) + square(d.y) + square(d.z)).saturating_sub(along2);
        // Radius 0.55 to 1 of the largest; depth in proportion.
        let size = 36_045 + (((a >> 16) * 29_491) >> 16) as i32;
        let radius = mul16(CRATER_RADIUS_Q19 as i32, size).max(1);
        let rad2 = mul_shr(radius as u32, radius as u32, 10).max(1);
        if r2 >= 4 * rad2 {
            continue;
        }
        // (r / radius)^2 in Q24, below 4 (the ejecta reach): a full-precision
        // reciprocal, so large craters move smoothly between columns.
        let x2 = squared_ratio(r2, rad2) as i32;
        let depth = scale(size, o.amplitude);
        let rim = scale(layer.a, depth);
        // The bowl rises as x^2; the ejecta falls as t^3 from the rim to
        // twice the radius; a smooth minimum rounds the rim between them.
        let bowl = mul_fine(x2, depth + rim) - depth;
        let t = (REACH - x2) / 3;
        let ejecta_height = mul_fine(mul_fine(mul_fine(t, t), t), rim);
        height += smooth_min(bowl, ejecta_height, rim / 4);
        if (hash3(cell.x, cell.y, cell.z, o.seed ^ 0x1B87_3593) & 0xffff) < layer.b as u32 {
            ejecta = ejecta.max(255 - ((x2 >> 16) * 255 / (REACH >> 16)) as u32);
        }
    }
    (height, ejecta)
}

/// Surface height (height units above the datum) of the column centred at
/// domain point `p` with a `2^level` reference cell footprint (single
/// rate: every octave at `p`).
pub fn height(k: &LandformConstants, maps: Option<MapView>, p: IVec3, level: u32) -> i32 {
    height_parts(k, maps, p, level).0
}

/// Surface word of a column (`terrain_column_nodes(..).y`), by material style.
/// Earthlike and rules: the erosion term as a signed byte of the erosion
/// amplitude sum (negative: gully floors, positive: the ribs between
/// them), [`RIVER_BED`] in a river's channel, [`ALLUVIUM`] on the planet
/// maps' sediment. Lunar: the freshest ejecta
/// (bits 0..6) and the basin flag (bit 7).
pub fn surface(k: &LandformConstants, maps: Option<MapView>, p: IVec3, level: u32) -> u32 {
    height_parts(k, maps, p, level).1
}

/// The surface word of a column in a river's channel (Earthlike and rules
/// styles; erosion bytes stop at -126).
pub const RIVER_BED: u32 = 0x80;
/// The surface word of a column on sediment (buried by the planet maps'
/// erosion at least [`ALLUVIUM_DEPTH`]).
pub const ALLUVIUM: u32 = 0x81;
pub const ALLUVIUM_DEPTH: i32 = 1_000;

/// `max(a, b)`, rounded over `k` like [`smooth_min`].
pub(crate) fn smooth_max(a: i32, b: i32, k: i32) -> i32 {
    -smooth_min(-a, -b, k)
}

/// Two-rate evaluation (`docs/two-rate-field.md`): octaves at least this
/// many shifts above a column's level (wavelengths of 256 node spacings
/// or more) are evaluated at nodes and interpolated, each column
/// evaluating only the finer ones. The nodes are the columns of a level
/// whose indices are multiples of `NODE_COLUMNS` (on their face: the face
/// grid is linear in its indices, so a column's weights between its four
/// nodes are exact), every column of a tile interpolating 5x5 of them.
pub const MACRO_GAP: u32 = 15;
/// Columns between nodes (16: a level cell's shift + 4).
pub const NODE_COLUMNS: i32 = 16;
/// Ridge octaves interpolated at most (each carries its noise and its
/// gradient; later ones are a column's own).
pub const MACRO_RIDGES: usize = 10;

/// Whether `o` (resolved at `level`) is a macro octave. Erosion and craters
/// never are: gully floors and crater rims are kinks interpolation would
/// blunt. Ridges are, by their noise (their crests are made per column).
#[inline]
fn macro_class(o: &Octave, level: u32) -> bool {
    o.shift >= level + MACRO_GAP && !matches!(o.class(), EROSION | CRATER)
}

/// What a node carries: the macro octaves' sums per layer (smooth), each
/// macro ridge octave's noise and gradient (in table order), and the macro
/// warp octaves' offset and Jacobian.
#[derive(Clone, Copy)]
struct Node {
    value: [i32; LAYERS],
    region: [i32; LAYERS],
    dvalue: [IVec3; LAYERS],
    dregion: [IVec3; LAYERS],
    ridges: [(i32, IVec3); MACRO_RIDGES],
    warp: IVec3,
    jacobian: [IVec3; 3],
}

impl Node {
    const ZERO: Self = Self {
        value: [0; LAYERS],
        region: [0; LAYERS],
        dvalue: [IVec3::ZERO; LAYERS],
        dregion: [IVec3::ZERO; LAYERS],
        ridges: [(0, IVec3::ZERO); MACRO_RIDGES],
        warp: IVec3::ZERO,
        jacobian: [IVec3::ZERO; 3],
    };
}

/// A warp octave's offset and Jacobian row at `p` (`erosion`: with the
/// Jacobian).
fn warp_octave(o: &Octave, p: IVec3, erosion: bool) -> (i32, IVec3) {
    if !erosion {
        return (
            mul_fine(o.amplitude, noise_fine(p, o.shift, o.seed)),
            IVec3::ZERO,
        );
    }
    let (n, d) = noise_fine_grad(p, o.shift, o.seed);
    let to_q24 = |v: i32| {
        let m = mul_fine(v, o.amplitude);
        if o.shift <= 24 {
            m << (24 - o.shift)
        } else {
            m >> (o.shift - 24)
        }
    };
    (
        mul_fine(o.amplitude, n),
        IVec3::new(to_q24(d.x), to_q24(d.y), to_q24(d.z)),
    )
}

/// The warp octaves of a stack.
fn warps(k: &LandformConstants) -> &[Octave] {
    &k.octaves[..(k.stack[3].max(0) as usize).min(WARP_OCTAVES)]
}

/// Whether any octave of `level` is a macro one.
fn has_macro(k: &LandformConstants, level: u32) -> bool {
    warps(k).iter().any(|o| o.shift >= level + MACRO_GAP)
        || k.table()
            .iter()
            .any(|o| resolved(o, level) && macro_class(o, level))
}

/// Per-level column programs: the ops a column of each reference level
/// runs, in table order, compiled once per stack, so that neither the CPU
/// nor the GPU walks the whole table per column (on the GPU the walk and
/// its checks were a sixth of a column's time). An op is its octave's index
/// (`OP_INDEX` bits) and flags; `LandformPlan` packs them for the GPU.
const OP_INDEX: u16 = 63;
/// A macro ridge: its noise from the nodes (`Node::ridges[rank]`, the rank
/// in the bits from `OP_RANK_SHIFT`).
const OP_MACRO: u16 = 1 << 6;
/// The display layer's ridge (generation's coarse levels keep the mean of
/// those they cannot resolve; the CPU field has no display variant).
const OP_DISPLAY: u16 = 1 << 7;
/// An unresolved display ridge: only the display variant runs it.
const OP_DISPLAY_ONLY: u16 = 1 << 8;
/// Past its layer's last region octave: the layer's mask is whole.
const OP_REGION_DONE: u16 = 1 << 9;
const OP_RANK_SHIFT: u16 = 10;
/// Changes what erosion steers by (continents and mountains): the next
/// erosion octave recomputes its steering.
const OP_STEER: u16 = 1 << 14;
/// Reference levels with a program of their own; coarser ones run the last
/// (no octave of the table is resolved finer than it).
pub const PLAN_LEVELS: usize = 32;
/// Ops of every level's program: at most each table octave once a level.
pub const PLAN_OPS: usize = PLAN_LEVELS * (OCTAVES - WARP_OCTAVES);
/// A level's node words in use: the value (bits 0..8), region (8..16) and
/// value gradient (16..24) of each layer, the warp, and whether there are
/// nodes at all.
const NODE_REGION_SHIFT: u32 = 8;
const NODE_DVALUE_SHIFT: u32 = 16;
const NODE_WARP: u32 = 1 << 24;
const NODE_ANY: u32 = 1 << 31;

/// The ops of `level`'s columns (`two_rate`: macro octaves from nodes).
fn level_ops(k: &LandformConstants, level: u32, two_rate: bool) -> Vec<u16> {
    let mut region_done = [0usize; LAYERS];
    for (n, o) in k.table().iter().enumerate() {
        if o.class() == REGION {
            region_done[o.layer()] = WARP_OCTAVES + n + 1;
        }
    }
    let display_tag = if k.shape[0] > 0 {
        (((k.shape[0] - 1) as u32) << 8) | RIDGE
    } else {
        u32::MAX
    };
    let mut ridges = 0;
    let mut ops = Vec::new();
    for (n, o) in k.table().iter().enumerate() {
        let index = WARP_OCTAVES + n;
        let res = resolved(o, level);
        let display = o.kind == display_tag;
        if !res && !display {
            continue;
        }
        let mut op = index as u16;
        if two_rate && res && macro_class(o, level) {
            if o.class() != RIDGE {
                continue;
            }
            ridges += 1;
            if ridges <= MACRO_RIDGES {
                op |= OP_MACRO | (((ridges - 1) as u16) << OP_RANK_SHIFT);
            }
        }
        if display {
            op |= OP_DISPLAY;
            if !res {
                op |= OP_DISPLAY_ONLY;
            }
        }
        if index >= region_done[o.layer()] {
            op |= OP_REGION_DONE;
        }
        let layer = k.layers[o.layer()].kind;
        if o.class() != EROSION
            && (layer == StackLayer::CONTINENTS || layer == StackLayer::MOUNTAINS)
        {
            op |= OP_STEER;
        }
        ops.push(op);
    }
    ops
}

/// The node words `level`'s columns read (`NODE_*`; 0: no nodes).
fn level_nodes(k: &LandformConstants, level: u32) -> u32 {
    if !has_macro(k, level) {
        return 0;
    }
    let mut m = NODE_ANY;
    if warps(k).iter().any(|o| o.shift >= level + MACRO_GAP) {
        m |= NODE_WARP;
    }
    for o in k.table() {
        if !resolved(o, level) || !macro_class(o, level) {
            continue;
        }
        let l = o.layer() as u32;
        match o.class() {
            CONTINENT => m |= (1 << l) | (1 << (NODE_DVALUE_SHIFT + l)),
            REGION => m |= 1 << (NODE_REGION_SHIFT + l),
            RIDGE => {}
            _ => m |= 1 << l,
        }
    }
    m
}

/// Every level's column program (`TerrainConstants::plan_*`): per level
/// its first op and op count (`first | count << 16`) and node words in
/// use, then the ops, two a word (the first in the low half).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct LandformPlan {
    lists: [u32; PLAN_LEVELS],
    nodes: [u32; PLAN_LEVELS],
    ops: [u32; PLAN_OPS / 2],
}

impl LandformPlan {
    pub fn new(k: &LandformConstants) -> Self {
        let mut plan = Self::zeroed();
        let mut at = 0usize;
        for level in 0..PLAN_LEVELS {
            let ops = level_ops(k, level as u32, true);
            plan.lists[level] = at as u32 | (ops.len() as u32) << 16;
            plan.nodes[level] = level_nodes(k, level as u32);
            for op in ops {
                plan.ops[at / 2] |= u32::from(op) << ((at & 1) * 16);
                at += 1;
            }
        }
        plan
    }

    /// The ops of reference level `level`'s columns.
    fn ops(&self, level: u32) -> impl Iterator<Item = u16> + '_ {
        let e = self.lists[(level as usize).min(PLAN_LEVELS - 1)];
        let first = (e & 0xffff) as usize;
        (first..first + (e >> 16) as usize).map(|n| (self.ops[n / 2] >> ((n & 1) * 16)) as u16)
    }
}

/// The node at lattice point `node` (domain units) of `level`: the warp
/// (all of it, to place the octaves; its macro octaves' part to carry),
/// then the macro octaves at the warped node.
fn macro_node(k: &LandformConstants, node: IVec3, level: u32, erosion: bool) -> Node {
    let mut out = Node::ZERO;
    let mut warp = [0i32; 3];
    for o in warps(k) {
        let axis = ((o.kind - WARP) as usize).min(2);
        let (w, j) = warp_octave(o, node, erosion);
        warp[axis] = warp[axis].wrapping_add(w);
        if o.shift >= level + MACRO_GAP {
            out.warp[axis] = out.warp[axis].wrapping_add(w);
            out.jacobian[axis] += j;
        }
    }
    let q = node + IVec3::from_array(warp);
    let mut ridges = 0;
    for o in k.table() {
        if !resolved(o, level) || !macro_class(o, level) {
            continue;
        }
        let l = o.layer();
        match o.class() {
            CONTINENT | REGION => {
                let (n, d) = if erosion {
                    noise_fine_grad(q, o.shift, o.seed)
                } else {
                    (noise_fine(q, o.shift, o.seed), IVec3::ZERO)
                };
                let v = mul_fine(n, o.amplitude << 8);
                let dv = mul_fine3(per_span3(d, o.shift), o.amplitude << 8);
                if o.class() == CONTINENT {
                    out.value[l] = out.value[l].wrapping_add(v);
                    out.dvalue[l] += dv;
                } else {
                    out.region[l] = out.region[l].wrapping_add(v);
                    out.dregion[l] += dv;
                }
            }
            RIDGE => {
                if ridges < MACRO_RIDGES {
                    out.ridges[ridges] = if erosion {
                        noise_fine_grad(q, o.shift, o.seed)
                    } else {
                        (noise_fine(q, o.shift, o.seed), IVec3::ZERO)
                    };
                }
                ridges += 1;
            }
            HILLS => {
                out.value[l] =
                    out.value[l].wrapping_add(mul_fine(o.amplitude, noise_fine(q, o.shift, o.seed)))
            }
            BASIN => out.value[l] = out.value[l].wrapping_add(noise_fine(node, o.shift, o.seed)),
            _ => {
                out.value[l] =
                    out.value[l].wrapping_add(scale(noise(node, o.shift, o.seed), o.amplitude))
            }
        }
    }
    out
}

/// The bilinear weights (sum 256) of a column `(di, dj)` columns past its
/// first node: node `c` is `(c & 1, c >> 1)` node spacings further.
fn node_weights(di: i32, dj: i32) -> [i32; 4] {
    let (u, v) = (NODE_COLUMNS - di, NODE_COLUMNS - dj);
    [u * v, di * v, u * dj, di * dj]
}

/// `floor(sum v_c w_c / 256)` (weights [`node_weights`]), wrapping: the high
/// and low bytes summed apart, so a node's words can be added one node at a
/// time (`landform.wgsl` does) and the result is exact.
#[inline]
fn lerp4(v: [i32; 4], w: [i32; 4]) -> i32 {
    let (mut hi, mut lo) = (0i32, 0i32);
    for c in 0..4 {
        hi = hi.wrapping_add((v[c] >> 8).wrapping_mul(w[c]));
        lo += (v[c] & 255) * w[c];
    }
    hi.wrapping_add(lo >> 8)
}

/// The four nodes around `column` (reference level `level`), interpolated
/// bilinearly at it.
fn macro_at(k: &LandformConstants, grid: &Grid, column: Column, level: u32, erosion: bool) -> Node {
    let (a, b) = (
        column.i & !(NODE_COLUMNS - 1),
        column.j & !(NODE_COLUMNS - 1),
    );
    let w = node_weights(column.i - a, column.j - b);
    let nodes: [Node; 4] = std::array::from_fn(|c| {
        let (i, j) = (
            a + NODE_COLUMNS * (c & 1) as i32,
            b + NODE_COLUMNS * (c >> 1) as i32,
        );
        macro_node(
            k,
            grid.domain_point(column.face, i, j, column.level),
            level,
            erosion,
        )
    });
    let bi = |v: [i32; 4]| lerp4(v, w);
    let bi3 =
        |v: [IVec3; 4]| IVec3::new(bi(v.map(|x| x.x)), bi(v.map(|x| x.y)), bi(v.map(|x| x.z)));
    let mut out = Node::ZERO;
    for l in 0..LAYERS {
        out.value[l] = bi(nodes.map(|n| n.value[l]));
        out.region[l] = bi(nodes.map(|n| n.region[l]));
        out.dvalue[l] = bi3(nodes.map(|n| n.dvalue[l]));
        out.dregion[l] = bi3(nodes.map(|n| n.dregion[l]));
    }
    for r in 0..MACRO_RIDGES {
        out.ridges[r] = (
            bi(nodes.map(|n| n.ridges[r].0)),
            bi3(nodes.map(|n| n.ridges[r].1)),
        );
    }
    out.warp = bi3(nodes.map(|n| n.warp));
    for axis in 0..3 {
        out.jacobian[axis] = bi3(nodes.map(|n| n.jacobian[axis]));
    }
    out
}

/// The height and surface word of `column`: its macro octaves from its
/// four nodes, the finer ones its own, by its level's program (the
/// terrain's definition, mirrored by `landform.wgsl`).
pub fn column_parts(
    k: &LandformConstants,
    maps: Option<MapView>,
    plan: &LandformPlan,
    grid: &Grid,
    column: Column,
) -> (i32, u32) {
    let level = column.level + grid.level_offset();
    let p = grid.domain_point(column.face, column.i, column.j, column.level);
    if has_macro(k, level) {
        let erosion = k.materials[2] >= (level + RESOLVED_SHIFT) as i32;
        parts(
            k,
            maps,
            p,
            level,
            Some(&macro_at(k, grid, column, level, erosion)),
            plan.ops(level),
        )
    } else {
        parts(k, maps, p, level, None, plan.ops(level))
    }
}

/// One ridge octave of noise `n` and gradient `d` into layer `l`'s chain.
fn ridge_step(s: &mut Accumulators, l: usize, o: &Octave, n: i32, d: IVec3, erosion: bool) {
    let r = (FINE_ONE - n.abs()).clamp(0, FINE_ONE - 1);
    let rr = mul_fine(r, r);
    let v = mul_fine(rr, s.weight[l]);
    if erosion {
        // The crest's sign flip is softened over |n| < 1/4.
        let dr = mul_fine3(per_span3(-d, o.shift), (n * 4).clamp(-FINE_ONE, FINE_ONE));
        let dv = mul_fine3(mul_fine3(dr, r) * 2, s.weight[l]) + mul_fine3(s.dweight[l], rr);
        s.dweight[l] = mul_fine3(dv * 2, soft_inside(v * 2, FINE_ONE / 4, FINE_ONE - 1));
        s.dvalue[l] += mul_fine3(dv, o.amplitude);
    }
    s.weight[l] = (v * 2).clamp(FINE_ONE / 4, FINE_ONE - 1);
    s.value[l] = s.value[l].wrapping_add(mul_fine(o.amplitude, v));
}

/// [`height`] and [`surface`] of the point `p` with every octave evaluated
/// there (the single-rate field; a column's is [`column_parts`]). Without
/// `maps` rivers carve nothing (the terrain the maps are baked from).
pub fn height_parts(
    k: &LandformConstants,
    maps: Option<MapView>,
    p: IVec3,
    level: u32,
) -> (i32, u32) {
    parts(
        k,
        maps,
        p,
        level,
        None,
        level_ops(k, level, false).into_iter(),
    )
}

/// [`height_parts`] of many points of one level without maps (the planet
/// maps' bake): the level's single-rate ops compiled once.
pub(crate) struct SingleRate {
    ops: Vec<u16>,
    level: u32,
}

impl SingleRate {
    pub(crate) fn new(k: &LandformConstants, level: u32) -> Self {
        Self {
            ops: level_ops(k, level, false),
            level,
        }
    }
    pub(crate) fn height(&self, k: &LandformConstants, p: IVec3) -> i32 {
        parts(k, None, p, self.level, None, self.ops.iter().copied()).0
    }
    /// [`Self::height`] with `maps` (the streams' routing: worn, not
    /// carved).
    pub(crate) fn height_in(&self, k: &LandformConstants, maps: Option<MapView>, p: IVec3) -> i32 {
        parts(k, maps, p, self.level, None, self.ops.iter().copied()).0
    }
}

/// The octaves of `ops` at `p` (reference level `level`), the macro ones
/// from `m` when given (interpolated: [`macro_at`]).
fn parts(
    k: &LandformConstants,
    maps: Option<MapView>,
    p: IVec3,
    level: u32,
    m: Option<&Node>,
    ops: impl Iterator<Item = u16>,
) -> (i32, u32) {
    // Erosion follows the slope of the larger terrain: when one of its
    // octaves is resolved here, the large octaves carry their gradients.
    let erosion = k.materials[2] >= (level + RESOLVED_SHIFT) as i32;
    // The warp, masks, ridges and hills scale up to kilometres, so they use
    // the fine (Q24) noise: 16-bit noise is constant over metres at these
    // wavelengths and its steps, multiplied by the relief, would cut
    // terraces between neighbouring columns.
    //
    // The warp is a coordinate transform, so every level evaluates it.
    // `jacobian[a]` is the gradient of warp axis `a` (Q24, dimensionless).
    // The macro octaves interpolated from the node lattice (none: every
    // octave the column's own).
    let two_rate = m.is_some();
    let m = m.copied().unwrap_or(Node::ZERO);
    let mut warp = m.warp.to_array();
    let mut jacobian = m.jacobian;
    for o in warps(k) {
        if two_rate && o.shift >= level + MACRO_GAP {
            continue;
        }
        let axis = ((o.kind - WARP) as usize).min(2);
        let (w, j) = warp_octave(o, p, erosion);
        warp[axis] = warp[axis].wrapping_add(w);
        jacobian[axis] += j;
    }
    let q = p + IVec3::from_array(warp);
    // Gradients w.r.t. q map to p through (I + J)^T.
    let unwarp = |g: IVec3| {
        let column = |j: usize| {
            g[j] + mul_fine(g.x, jacobian[0][j])
                + mul_fine(g.y, jacobian[1][j])
                + mul_fine(g.z, jacobian[2][j])
        };
        IVec3::new(column(0), column(1), column(2))
    };
    let up = if k.shape[3] != 0 {
        IVec3::new(0, Q30 as i32, 0)
    } else {
        unit_q30(p)
    };
    // The macro octaves' sums come first; their ridges join the chains in
    // table order below.
    let mut s = Accumulators {
        value: m.value,
        region: m.region,
        weight: [FINE_ONE - 1; LAYERS],
        dvalue: m.dvalue,
        dregion: m.dregion,
        dweight: [IVec3::ZERO; LAYERS],
    };
    let mut eroded = 0i32;
    let mut deroded = IVec3::ZERO;
    let mut ejecta = 0u32;
    // Steering reads only continents and mountains (`OP_STEER` ops change
    // them): the erosion octaves between share one.
    let mut steer = None;
    for op in ops {
        if op & OP_DISPLAY_ONLY != 0 {
            continue;
        }
        let o = &k.octaves[usize::from(op & OP_INDEX)];
        if op & OP_STEER != 0 {
            steer = None;
        }
        let l = o.layer();
        if op & OP_MACRO != 0 {
            let (n, d) = m.ridges[usize::from((op >> OP_RANK_SHIFT) & 15)];
            ridge_step(&mut s, l, o, n, d, erosion);
            continue;
        }
        match o.class() {
            CONTINENT | REGION => {
                let (n, d) = if erosion {
                    noise_fine_grad(q, o.shift, o.seed)
                } else {
                    (noise_fine(q, o.shift, o.seed), IVec3::ZERO)
                };
                let v = mul_fine(n, o.amplitude << 8);
                let dv = mul_fine3(per_span3(d, o.shift), o.amplitude << 8);
                if o.class() == CONTINENT {
                    s.value[l] = s.value[l].wrapping_add(v);
                    s.dvalue[l] += dv;
                } else {
                    s.region[l] = s.region[l].wrapping_add(v);
                    s.dregion[l] += dv;
                }
            }
            RIDGE => {
                let (n, d) = if erosion {
                    noise_fine_grad(q, o.shift, o.seed)
                } else {
                    (noise_fine(q, o.shift, o.seed), IVec3::ZERO)
                };
                ridge_step(&mut s, l, o, n, d, erosion);
            }
            HILLS => {
                s.value[l] =
                    s.value[l].wrapping_add(mul_fine(o.amplitude, noise_fine(q, o.shift, o.seed)))
            }
            EROSION => {
                let g = *steer.get_or_insert_with(|| unwarp(steering(k, &s))) + deroded;
                let (e, de) = erosion_octave(k.layers[l].a, o, p, g, up);
                s.value[l] = s.value[l].wrapping_add(e);
                eroded = eroded.wrapping_add(e);
                deroded += de;
            }
            CRATER => {
                let (h, e) = crater_octave(k, &k.layers[l], o, p, up);
                s.value[l] = s.value[l].wrapping_add(h);
                ejecta = ejecta.max(e);
            }
            BASIN => s.value[l] = s.value[l].wrapping_add(noise_fine(p, o.shift, o.seed)),
            _ => {
                s.value[l] = s.value[l].wrapping_add(scale(noise(p, o.shift, o.seed), o.amplitude))
            }
        }
    }
    // Compose the layers in stack order.
    let c = k.continents().map_or(FINE_ONE, |l| s.value[l]);
    let (land, wet) = land_masks(c);
    let mut h = 0i32;
    let mut basin = 0i32;
    let mut river_bed = false;
    let mut alluvium = false;
    for (l, layer) in k.stack_layers().iter().enumerate() {
        let x = match layer.kind {
            StackLayer::RIVERS => {
                // The worn terrain (cut down, buried under sediment), then
                // valleys and channels along the river segments, cut into
                // what lies below them in the stack.
                if let Some(m) = maps {
                    if m.header[0][0] & planet_maps::FLAG_EROSION != 0 {
                        let (cut, fill) = planet_maps::erosion(m, p);
                        let worn = h.wrapping_add(cut);
                        h = smooth_max(worn, fill, m.header[4][2]);
                        alluvium = fill > worn.wrapping_add(ALLUVIUM_DEPTH);
                    }
                    let (carved, bed) = planet_maps::carve(m, p, level, h);
                    h = carved;
                    river_bed |= bed;
                    // The streams below the maps' texels.
                    if let Some(s) = m.streams {
                        let (carved, bed) = s.carve(m, p, level, h);
                        h = carved;
                        river_bed |= bed;
                    }
                }
                continue;
            }
            StackLayer::CONTINENTS => continent_base(c, layer.a, layer.b),
            StackLayer::MOUNTAINS => mul_fine(s.value[l], region_weight(s.region[l], layer.a)),
            StackLayer::BASINS => {
                // Basins flatten what lies below them in the stack by half
                // and sink it by their depth.
                let m = masked(
                    layer.mask,
                    (s.value[l].wrapping_sub(layer.b).wrapping_mul(4)).clamp(0, FINE_ONE),
                    land,
                    wet,
                );
                basin = basin.max(m);
                h = mul_fine(h, FINE_ONE - m / 2).wrapping_sub(mul_fine(layer.a, m));
                continue;
            }
            StackLayer::PLATEAU => layer.a,
            StackLayer::HILLS
            | StackLayer::ROUGHNESS
            | StackLayer::EROSION
            | StackLayer::CRATERS => s.value[l],
            _ if layer.base_kind() == StackLayer::CLIFFS => {
                // Escarpments: inside their regions, terraces of what lies
                // below them in the stack.
                let m = masked(
                    layer.mask,
                    (s.value[l].wrapping_sub(layer.b).wrapping_mul(4)).clamp(0, FINE_ONE),
                    land,
                    wet,
                );
                h = h.wrapping_add(mul_fine(
                    terrace(h, layer.a, (layer.kind >> 16) as i32).wrapping_sub(h),
                    m,
                ));
                continue;
            }
            _ => 0,
        };
        h = h.wrapping_add(masked(layer.mask, x, land, wet));
    }
    let surface = match k.style[0] {
        STYLE_EARTHLIKE | STYLE_RULES if river_bed => RIVER_BED,
        STYLE_EARTHLIKE | STYLE_RULES if alluvium => ALLUVIUM,
        STYLE_EARTHLIKE | STYLE_RULES => {
            ((eroded * 126 / k.stack[1].max(1)).clamp(-126, 126) as u32) & 0xff
        }
        STYLE_LUNAR => (ejecta >> 1) | (u32::from(basin >= FINE_ONE / 2) << 7),
        _ => 0,
    };
    (h, surface)
}

/// Snowline (mm) where the sea-level temperature is `t0` (1/128 °C): the
/// height at which the lapse rate brings it to the snow temperature.
fn snowline(c: &LandformConstants, t0: i32) -> i32 {
    let (snow, per) = (c.maps[3][0], c.maps[3][1]);
    if per == 0 {
        return if t0 > snow {
            i32::MAX / 2
        } else {
            i32::MIN / 2
        };
    }
    (t0 - snow).clamp(-8192, 8192) * per
}

/// The Earthlike snowline's spread: moisture moves it by up to 375 m,
/// noise by 500 m over ~1.6 km, and the alpine rock band under it is 500 m.
const SNOW_WET: i32 = 750_000;
const SNOW_BAND: i32 = 500_000;

/// Strata altitude (mm): layers undulate +-8 m over ~100 m, so cuts
/// through them never show flat rings.
fn strata(c: &LandformConstants, p: IVec3, altitude: i32) -> i32 {
    altitude + scale(noise(p, 13, (c.header[3] as u32) ^ 0x9B05_688C), 8_000)
}

/// Earthlike materials: meadows, dry lands, rock outcrops, strata and snow
/// above the snowline (the climate's: latitude and height); gravel in
/// erosion gullies and river beds.
fn earthlike_material(
    c: &LandformConstants,
    maps: MapView,
    p: IVec3,
    surface: u32,
    top_height: i32,
    depth: i32,
    slope: i32,
    layer: i32,
) -> u32 {
    use crate::terrain::material::*;
    let dirt = c.header[2];
    let steep = slope >= c.shape[1];
    let (t0, wet) = planet_maps::climate(maps, p, 0);
    let wet = wet << 8;
    // Hash every domain axis: on a face one of them is nearly constant.
    let h = hash3(p.x, p.y, p.z ^ layer.wrapping_mul(0x9e37), 0x2545_F491);
    let altitude = layer.wrapping_mul(c.header[1]);
    if surface == RIVER_BED && depth < dirt * 2 {
        // River beds: gravel and sand over silt.
        return if depth == 0 && h & 3 == 0 {
            SAND
        } else if depth < dirt {
            GRAVEL
        } else {
            CLAY
        };
    }
    if top_height < c.levels[3] {
        // Low basins: meadow with mud patches of a few metres over silt,
        // gravel and stone. Patches follow position noise, never height
        // contours. (Single-cell mud and sand specks every few cells read as
        // noise that hid the ground's shape.)
        if depth == 0 {
            let mud = noise(p, 8, (c.header[3] as u32) ^ 0x5f35_6495);
            return if mud > ONE / 2 { DIRT } else { GRASS };
        }
        return if depth < dirt * 2 {
            if h & 3 == 0 {
                GRAVEL
            } else {
                CLAY
            }
        } else {
            STONE
        };
    }
    // The snowline wanders 500 m over ~1.6 km (a level snowline drew a
    // ruler-straight edge along every range), and the alpine rock band
    // below it is 500 m: meadows climb.
    let wander = scale(noise(p, 17, (c.header[3] as u32) ^ 0x3C6E_F372), SNOW_BAND);
    let snowline = snowline(c, t0) + mul_fine(SNOW_WET, wet - FINE_ONE / 2) + wander;
    // Alpine weight: 0 below the rockline, ONE at the snowline.
    let rockline = snowline - SNOW_BAND;
    let band = SNOW_BAND;
    let alpine = ((top_height - rockline).clamp(0, band) / 256) * ONE / (band / 256);
    // Rock patches (~50 m and ~6 m octaves), also breaking up snow edges.
    // Noise is clamped to +-ONE, so below the rockline on gentler slopes no
    // outcrop can reach the rock fringe: skip it (same result).
    let seed = c.header[3] as u32;
    let outcrop = if alpine > 0 || slope >= 5 {
        noise(p, 12, seed ^ 0x1B56_C4E9) + noise(p, 9, seed ^ 0x6A09_E667) / 3
    } else {
        -ONE
    };
    // Snow does not hold on faces steeper than ~37 degrees: rock streaks the snowfields.
    if top_height > snowline && depth < dirt && slope + outcrop / 8192 < 6 {
        return SNOW;
    }
    if surface == ALLUVIUM && !steep && depth < dirt * 3 {
        // Sediment: dry fans of sand and gravel, wet plains of meadow over
        // soil and silt.
        return if wet < FINE_ONE / 10 * 3 {
            if depth == 0 && h & 3 != 0 {
                SAND
            } else {
                GRAVEL
            }
        } else if depth == 0 {
            GRASS
        } else if depth < dirt {
            DIRT
        } else {
            CLAY
        };
    }
    if wet < FINE_ONE / 10 * 3 {
        // Dry lands: sand over banded sandstone and clay.
        if depth < dirt && !steep {
            return SAND;
        }
        let band = strata(c, p, altitude).div_euclid(2_100).rem_euclid(5);
        return if band == 1 || band == 3 {
            CLAY
        } else {
            SANDSTONE
        };
    }
    // Rock shows through the turf in the outcrop patches, which grow up the
    // alpine band below the snowline and on hillsides over ~32 degrees; scree and
    // bare soil fringe them. Deeper cells keep the strata.
    let mut exposed = outcrop + 2 * alpine - ONE;
    if slope >= 5 {
        exposed += ONE / 2;
    }
    // Erosion on slopes: rock shows on the ribs between gullies; sediment
    // fills their floors.
    let gully = i32::from(surface as u8 as i8);
    if slope >= 2 {
        exposed += gully * (ONE / 256);
    }
    if steep || (exposed > 0 && depth < dirt) {
        return if depth < 1 && h & 7 == 0 {
            DIRT
        } else if (altitude + scale(outcrop, 3_000)).div_euclid(4_500) & 1 == 0 {
            STONE
        } else {
            DARK_STONE
        };
    }
    if depth == 0 {
        if exposed > -ONE / 16 || (gully < -64 && slope >= 3) {
            GRAVEL
        } else if exposed > -ONE / 8 {
            DIRT
        } else {
            GRASS
        }
    } else if depth < dirt {
        DIRT
    } else if depth < dirt * 3 && h & 3 == 0 {
        GRAVEL
    } else if strata(c, p, altitude).div_euclid(12_000) & 1 == 0 {
        STONE
    } else {
        DARK_STONE
    }
}

/// Material of a solid ground cell by the stack's material style.
/// `top_height` is the column height (mm), `depth` cells below the column
/// top (0 = exposed top cell), `slope` the ground slope in eighths of a
/// cell per cell (`Planet::material_slope`), `layer` the base layer index
/// of the cell; `p` is the cell's volume point.
pub fn ground_material(
    c: &LandformConstants,
    maps: MapView,
    p: IVec3,
    surface: u32,
    top_height: i32,
    depth: i32,
    slope: i32,
    layer: i32,
) -> u32 {
    match c.style[0] {
        STYLE_LUNAR => lunar_material(c, p, surface, depth, slope, layer),
        STYLE_RULES => rules_material(c, maps, p, surface, top_height, depth, slope, layer),
        STYLE_LAYERED => {
            (if depth == 0 {
                c.style[1]
            } else if depth <= c.header[2] {
                c.style[2]
            } else {
                c.style[3]
            }) as u32
        }
        _ => earthlike_material(c, maps, p, surface, top_height, depth, slope, layer),
    }
}

/// The first rule whose conditions hold, else the fallback material.
/// Moisture and temperature are the climate's at the cell (its altitude);
/// a river bed counts as the deepest gully. Patches are noise above a
/// threshold (their coverage); strata bands follow the undulating altitude
/// (`strata`); specks are single cells flagged
/// [`crate::terrain::material::SPECK`] on the surface.
#[allow(clippy::too_many_arguments)]
fn rules_material(
    c: &LandformConstants,
    maps: MapView,
    p: IVec3,
    surface: u32,
    top_height: i32,
    depth: i32,
    slope: i32,
    layer: i32,
) -> u32 {
    let river = surface == RIVER_BED;
    // Sediment is no gully.
    let erosion = if surface == ALLUVIUM {
        0
    } else {
        i32::from(surface as u8 as i8).max(-126)
    };
    let altitude = layer.wrapping_mul(c.header[1]);
    let (temperature, wet) = planet_maps::climate(maps, p, altitude);
    let seed = c.header[3] as u32;
    let count = (c.materials[0].max(0) as usize).min(RULES);
    for (index, r) in c.rules[..count].iter().enumerate() {
        let within = |v: i32, lo: i32, hi: i32| v >= lo && v <= hi;
        if !within(top_height, r.height_slope[0], r.height_slope[1])
            || !within(slope, r.height_slope[2], r.height_slope[3])
            || !within(depth, r.depth_moisture[0], r.depth_moisture[1])
            || !within(wet, r.depth_moisture[2], r.depth_moisture[3])
            || !within(erosion, r.surface_bands[0], r.surface_bands[1])
            || !within(temperature, r.climate[0], r.climate[1])
            || (r.climate[2] == 1 && !river)
            || (r.climate[2] == 2 && river)
        {
            continue;
        }
        let salt = (index as u32).wrapping_mul(0x9E37_79B9);
        if r.surface_bands[2] > 0
            && strata(c, p, altitude).div_euclid(r.surface_bands[2]) & 1 != r.surface_bands[3]
        {
            continue;
        }
        if r.head[2] > 0 && noise(p, r.head[2] as u32, seed ^ 0x68E3_1DA4 ^ salt) <= r.head[3] {
            continue;
        }
        if r.head[1] > 0 {
            if (hash3(
                p.x,
                p.y,
                p.z ^ layer.wrapping_mul(0x9e37),
                0x5f35_6495 ^ salt,
            ) & 255) as i32
                >= r.head[1]
            {
                continue;
            }
            return r.head[0] as u32
                | if depth == 0 {
                    crate::terrain::material::SPECK
                } else {
                    0
                };
        }
        return r.head[0] as u32;
    }
    c.materials[1] as u32
}

/// Lunar materials: ids into [`lunar_appearance`].
pub mod lunar {
    pub use crate::terrain::material::{
        ANORTHOSITE, BASALT, BOULDER as ROCK, EJECTA, MARE, REGOLITH,
    };
}

/// Regolith over bedrock, dark basin plains, bright young ejecta thinning
/// out away from the crater, boulders on steep slopes.
fn lunar_material(
    c: &LandformConstants,
    p: IVec3,
    surface: u32,
    depth: i32,
    slope: i32,
    layer: i32,
) -> u32 {
    use lunar::*;
    let ejecta = (surface & 0x7f) << 1;
    let mare = surface & 0x80 != 0;
    let h = hash3(p.x, p.y, p.z ^ layer.wrapping_mul(0x9e37), 0x2545_F491);
    if depth == 0 {
        if slope >= 12 || (slope >= 6 && h & 3 == 0) {
            return ROCK;
        }
        if ejecta > (h & 0xff) {
            return EJECTA;
        }
        return if mare { MARE } else { REGOLITH };
    }
    if depth < c.header[2] {
        return if mare { MARE } else { REGOLITH };
    }
    if mare {
        BASALT
    } else {
        ANORTHOSITE
    }
}

/// The lunar look: the shared material table (lunar ground has its own
/// ids there), less patch and pigment contrast, steps lit mostly by the
/// slope they stand for: lit by their own faces, a crater bowl's 10 cm
/// terraces drew concentric rings (moiré at distance), their shadows black
/// without a sky to fill them.
pub fn lunar_appearance() -> TerrainAppearance {
    TerrainAppearance {
        detail: [0.2, 0.12, 0.08, 0.75],
        ..TerrainAppearance::default()
    }
}

/// A compiled terrain stack on one grid, with its planet maps.
pub struct LandformField {
    grid: Grid,
    constants: LandformConstants,
    volumes: Option<VolumeLayer>,
    bounds: [i32; 24],
    ridge_suffix: Option<[[i32; 4]; 66]>,
    plan: Box<LandformPlan>,
    maps: Option<Arc<PlanetMaps>>,
    streams: Option<Arc<crate::streams::Streams>>,
}

impl LandformField {
    /// The field of `constants` (their uniform climate in `maps`' rows
    /// unless baked `maps` replace them).
    pub fn new(
        grid: &Grid,
        mut constants: LandformConstants,
        volumes: Option<VolumeLayer>,
        maps: Option<Arc<PlanetMaps>>,
    ) -> Self {
        let streams = maps.as_ref().and_then(|m| {
            let r = m.rivers()?;
            constants.maps = m.header;
            (r.streams > 0).then(|| {
                Arc::new(crate::streams::Streams::new(
                    m.clone(),
                    &constants,
                    r,
                    r.streams,
                ))
            })
        });
        if let Some(m) = &maps {
            constants.maps = m.header;
            if let Some(s) = &streams {
                constants.maps[0][0] |= planet_maps::FLAG_STREAMS | (s.levels() as i32) << 8;
            }
            let (mut cut, mut slope) = m.valley_bounds();
            if let Some(s) = &streams {
                let (a, b) = s.bounds();
                cut = cut.saturating_add(a);
                slope = slope.saturating_add(b);
            }
            for layer in constants.layers.iter_mut() {
                if layer.kind == StackLayer::RIVERS {
                    (layer.a, layer.b) = (cut, slope);
                }
            }
        }
        let mut bounds = constants.bound_margins(grid);
        // Stacks without a supported display layer keep the canonical path.
        // In particular, do not saturate invalid huge amplitudes into new
        // terrain.
        let ridge_suffix = crate::ridge_envelope::bake_ridge_suffix(grid, &constants).ok();
        // Cliff shapes raise a finer column's highest solid cell over its
        // heightfield top.
        if let Some(v) = &volumes {
            for level in 0..24u32 {
                bounds[level as usize] = bounds[level as usize].saturating_add(v.rise_cells(level));
            }
        }
        let plan = Box::new(LandformPlan::new(&constants));
        Self {
            grid: *grid,
            bounds,
            ridge_suffix,
            constants,
            volumes,
            plan,
            maps,
            streams,
        }
    }
    pub fn constants(&self) -> &LandformConstants {
        &self.constants
    }
    /// The baked planet maps, if the stack has any.
    pub fn maps(&self) -> Option<&Arc<PlanetMaps>> {
        self.maps.as_ref()
    }
    /// The refined drainage, if the stack's rivers have streams.
    pub fn streams(&self) -> Option<&crate::streams::Streams> {
        self.streams.as_deref()
    }
    /// What climate and river lookups read.
    pub fn map_view(&self) -> MapView<'_> {
        MapView {
            header: &self.constants.maps,
            tables: self.maps.as_ref().map_or(&[], |m| &m.tables),
            streams: self.streams.as_deref(),
        }
    }
}

impl TerrainField for LandformField {
    fn height(&self, column: Column) -> i32 {
        column_parts(
            &self.constants,
            Some(self.map_view()),
            &self.plan,
            &self.grid,
            column,
        )
        .0
    }
    /// Streams only cut (and the layers after them keep the order), so the
    /// field without them bounds it from above.
    fn height_bound(&self, column: Column) -> i32 {
        let view = MapView {
            streams: None,
            ..self.map_view()
        };
        column_parts(&self.constants, Some(view), &self.plan, &self.grid, column).0
    }
    fn ground_material(
        &self,
        p: IVec3,
        surface: u32,
        top_height: i32,
        depth: i32,
        slope: i32,
        layer: i32,
    ) -> u32 {
        ground_material(
            &self.constants,
            self.map_view(),
            p,
            surface,
            top_height,
            depth,
            slope,
            layer,
        )
    }
    fn surface(&self, column: Column, _height: i32) -> u32 {
        column_parts(
            &self.constants,
            Some(self.map_view()),
            &self.plan,
            &self.grid,
            column,
        )
        .1
    }
    fn climate(&self, p: IVec3, height: i32) -> (i32, i32) {
        planet_maps::climate(self.map_view(), p, height)
    }
    fn erosion(&self, p: IVec3) -> (i32, i32) {
        planet_maps::erosion(self.map_view(), p)
    }
    fn drainage(&self, p: IVec3) -> [i32; 4] {
        planet_maps::drainage(self.map_view(), p)
    }
    fn height_range(&self) -> (i32, i32) {
        self.constants.height_range()
    }
    fn bound_margins(&self) -> [i32; 24] {
        self.bounds
    }
    fn volumes(&self) -> Option<&VolumeLayer> {
        self.volumes.as_ref()
    }
    fn appearance(&self) -> TerrainAppearance {
        if self.constants.style[0] == STYLE_LUNAR {
            lunar_appearance()
        } else {
            TerrainAppearance::default()
        }
    }
    fn program(&self) -> TerrainProgram {
        let mut canonical = self.constants;
        canonical.shape[2] = i32::from(self.ridge_suffix.is_some());
        let mut constants = bytemuck::bytes_of(&canonical).to_vec();
        constants.extend_from_slice(bytemuck::cast_slice(
            &self.ridge_suffix.unwrap_or([[0; 4]; 66]),
        ));
        constants.extend_from_slice(bytemuck::bytes_of(&*self.plan));
        TerrainProgram {
            key: Cow::Borrowed(DISPLAY_PROGRAM),
            wgsl: Cow::Borrowed(concat!(
                include_str!("../shaders/landform.wgsl"),
                include_str!("../shaders/planet_maps.wgsl")
            )),
            constants,
            tables: self
                .maps
                .as_ref()
                .map_or_else(|| Arc::from(Vec::new()), |m| Arc::from(m.tables.as_slice())),
            regions: self
                .streams
                .clone()
                .map(|s| crate::terrain::Regions(s as Arc<dyn crate::terrain::TerrainRegions>)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two-rate field against every octave at the column: within a
    /// fraction of a level cell everywhere (wavelengths of 256 node
    /// spacings interpolate to millimetres; ridge crests stay sharp, made
    /// per column from interpolated noise).
    #[test]
    fn two_rate_field_stays_within_a_fraction_of_a_cell() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let field = crate::layers::TerrainLayers::earth()
            .field(&grid, 75)
            .unwrap();
        let (k, maps) = (field.constants(), Some(field.map_view()));
        let plan = LandformPlan::new(k);
        let n = f64::from(grid.cells());
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng >> 11) as f64 / (1u64 << 53) as f64
        };
        for level in 0..13u32 {
            let cell = (HEIGHT_ONE as f64 * 0.1) * f64::from(1u32 << level);
            let mut worst = 0.0f64;
            for _ in 0..400 {
                let (fi, fj) = (0.3 + 0.4 * next(), 0.3 + 0.4 * next());
                let column = Column {
                    face: 2,
                    i: (fi * n) as i32 >> level,
                    j: (fj * n) as i32 >> level,
                    level,
                };
                let a = column_parts(k, maps, &plan, &grid, column).0;
                let b = height_parts(
                    k,
                    maps,
                    grid.domain_point(column.face, column.i, column.j, column.level),
                    level + grid.level_offset(),
                )
                .0;
                worst = worst.max(f64::from((a - b).abs()) / cell);
            }
            eprintln!("TWO_RATE level {level}: worst {worst:.4} cells");
            assert!(worst < 0.25, "level {level}: {worst} cells");
        }
    }
    use crate::layers::TerrainLayers;

    fn earth(grid: &Grid) -> LandformField {
        TerrainLayers::earth().field(grid, 7).unwrap()
    }

    #[test]
    fn climate_bounds_cover_both_signs_of_canonical_height_change() {
        for voxel in [0.1, 0.3, 1.0] {
            let grid = Grid::new(6_371_000.0, voxel).unwrap();
            let field = earth(&grid);
            let (k, maps) = (field.constants(), Some(field.map_view()));
            let bounds = k.bound_margins(&grid);
            let mut rng = 0x2545_F491_4F6C_DD1Du64;
            let mut next = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                rng
            };
            for _ in 0..5000 {
                let level = 1 + (next() % u64::from(grid.levels() - 1)) as u32;
                let face = (next() % 6) as u8;
                let cells = (grid.cells() >> level).max(1) as u64;
                let i = (next() % cells) as i32;
                let j = (next() % cells) as i32;
                let coarse = height(
                    k,
                    maps,
                    grid.domain_point(face, i, j, level),
                    level + grid.level_offset(),
                );
                let top = coarse.div_euclid(k.header[1]) >> level;
                let fi = (i << level) + (next() % (1u64 << level)) as i32;
                let fj = (j << level) + (next() % (1u64 << level)) as i32;
                let canonical = i64::from(height(
                    k,
                    maps,
                    grid.domain_point(face, fi, fj, 0),
                    grid.level_offset(),
                ));
                let cell_mm = i64::from(k.header[1]) * (1i64 << level);
                let lo = i64::from(top - bounds[level as usize]) * cell_mm;
                let hi = i64::from(top + bounds[level as usize]) * cell_mm;
                assert!(
                    (lo..=hi).contains(&canonical),
                    "{voxel} m level {level}: {canonical} outside {lo}..={hi}"
                );
            }
        }
    }

    #[test]
    fn height_range_is_planetary_and_levels_agree_on_large_scale() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let field = earth(&grid);
        let (k, maps) = (field.constants(), Some(field.map_view()));
        let (range_lo, range_hi) = k.height_range();
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        let n = grid.cells();
        let offset = grid.level_offset();
        for s in 0..400 {
            let i = (s * 7919 % 400) * (n / 400);
            let j = (s * 104_729 % 400) * (n / 400);
            let p = grid.domain_point((s % 6) as u8, i, j, 0);
            let h0 = height(k, maps, p, offset);
            let h12 = height(k, maps, p, 12 + offset);
            lo = lo.min(h0);
            hi = hi.max(h0);
            // Band limiting only removes octaves shorter than the level.
            let metres = f64::from((h0 - h12).abs()) / 1000.0;
            assert!(metres < 400.0, "{metres}");
        }
        let lo_m = f64::from(lo) / 1000.0;
        let hi_m = f64::from(hi) / 1000.0;
        assert!(
            lo_m < -200.0 && hi_m > 100.0 && hi_m < 6_000.0,
            "{lo_m} {hi_m}"
        );
        assert!(range_lo <= lo && hi <= range_hi);
    }

    /// Neighbouring columns never step by a voxel layer: the second
    /// difference of heights across three adjacent 0.1 m columns stays below
    /// 100 mm. Low-frequency weights multiply kilometres of relief, so with
    /// 16-bit noise their quantization cut straight terraces up to a metre
    /// high across the land (1791 of these samples). What remains is the
    /// integer domain warp moving a steep slope by one extra domain unit
    /// (0.05 m), under 80 mm.
    #[test]
    fn the_field_has_no_steps_between_neighbouring_columns() {
        let grid = Grid::new(6_371_000.0, 0.1).unwrap();
        let field = earth(&grid);
        let (k, maps) = (field.constants(), Some(field.map_view()));
        let n = grid.cells() as u64;
        let mut rng = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let offset = grid.level_offset();
        let (mut worst, mut steps) = (0, 0);
        for s in 0..20_000 {
            let face = (s % 6) as u8;
            let (i, j) = ((next() % (n - 4)) as i32, (next() % (n - 4)) as i32);
            // Along i and along j.
            for d in [(1, 0), (0, 1)] {
                let h = |t: i32| {
                    height(
                        k,
                        maps,
                        grid.domain_point(face, i + d.0 * t, j + d.1 * t, 0),
                        offset,
                    )
                };
                let second = (h(0) - 2 * h(1) + h(2)).abs();
                worst = worst.max(second);
                steps += usize::from(second >= 100);
            }
        }
        assert!(
            steps == 0,
            "{steps} steps, worst second difference {worst} mm"
        );
    }
}
