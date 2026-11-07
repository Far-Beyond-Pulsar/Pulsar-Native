//! Baked real-lens prescriptions on the GPU: the [`PrescriptionBank`] a host
//! registers as [`crate::PRESCRIPTIONS`] and the
//! [`element_kind::PRESCRIPTION`](crate::element_kind::PRESCRIPTION) elements
//! index by layer.
//!
//! The bank is one read-only storage buffer of 32-bit words (floats are stored
//! as their bits) so every lens keeps its own table sizes: a directory, then a
//! fixed header per lens, then the tables. `lens.wgsl` mirrors [`word`].
//!
//! ```text
//! [count, header offset of lens 0, 1, ...]  headers (HEADER_WORDS each)  tables
//! ```
//!
//! What is uploaded of a [`helio_lens_optics::LensBake`], and what it drives:
//!
//! | Bake | Uploaded | Drives |
//! | --- | --- | --- |
//! | `ray_map` | `tan_theta` per wavelength and field | ray map: distortion and lateral CA |
//! | `vignetting` | the whole table | transmission |
//! | `footprints` | masks and fields | pupil shape (cat's eye) |
//! | `focus` | per-channel axial focus, field curvature | focus field |
//! | `psf` | RMS ellipse and coma at each field's best focus | the aberration stage |

use crate::ray_map::sensor_weights;
use helio_lens_optics::{LensBake, PsfTable};

/// Optional registry entry: the storage buffer of a [`PrescriptionBank`] for
/// `element_kind::PRESCRIPTION` elements. Without it a bank of no lenses is
/// bound, so such an element covers nothing.
pub const PRESCRIPTIONS: &str = "lens_prescriptions";

/// Offsets (in words) into a lens's header; `lens.wgsl` has the same list.
pub(crate) mod word {
    pub const EFL: usize = 0;
    pub const F_MIN: usize = 1;
    pub const HALF_DIAGONAL: usize = 2;
    pub const FLAGS: usize = 3;
    pub const RAY_FIELDS: usize = 4;
    pub const RAY_WAVES: usize = 5;
    pub const RAY_WAVELENGTHS: usize = 6;
    pub const RAY_TAN: usize = 7;
    pub const VIG_FIELDS: usize = 8;
    pub const VIG_F_NUMBERS: usize = 9;
    pub const VIG_F_NUMBER_LIST: usize = 10;
    pub const VIG_DATA: usize = 11;
    pub const FOOTPRINT_RESOLUTION: usize = 12;
    pub const FOOTPRINT_FIELDS: usize = 13;
    pub const FOOTPRINT_FIELD_LIST: usize = 14;
    pub const FOOTPRINT_DATA: usize = 15;
    pub const FOOTPRINT_F_NUMBER: usize = 16;
    pub const FOCUS_RED: usize = 17;
    pub const ANALYSIS_FIELDS: usize = 20;
    pub const ANALYSIS_DATA: usize = 21;
    pub const LEN: usize = 24;
}

/// Words of one analysis record: field radius, field curvature (dioptres),
/// radial and tangential Gaussian sigma (mm), signed coma share, coma length (mm).
pub(crate) const ANALYSIS_STRIDE: usize = 6;
/// [`word::FLAGS`]: the wavelengths' ray maps differ.
pub(crate) const FLAG_CHROMATIC: u32 = 1;
/// [`word::FLAGS`]: the lens has an in-focus point spread worth applying.
pub(crate) const FLAG_PSF: u32 = 2;

/// Sigma (mm) under which a spot counts as perfect: an ideal lens traces to
/// rounding error.
const PSF_EPSILON_MM: f32 = 1.0e-4;
/// Relative `tan_theta` difference between wavelengths under which the
/// lens counts as achromatic.
const CHROMATIC_EPSILON: f32 = 1.0e-6;

fn push_f32(words: &mut Vec<u32>, values: impl IntoIterator<Item = f32>) {
    words.extend(values.into_iter().map(f32::to_bits));
}

/// `(lower, upper, t)` so that `xs[lower] + t (xs[upper] - xs[lower]) = x`,
/// clamped to the range of the ascending list.
fn bracket(xs: &[f32], x: f32) -> (usize, usize, f32) {
    let last = xs.len().saturating_sub(1);
    if last == 0 || x <= xs[0] {
        return (0, 0, 0.0);
    }
    if x >= xs[last] {
        return (last, last, 0.0);
    }
    let upper = xs.partition_point(|&v| v <= x).min(last);
    let lower = upper - 1;
    (
        lower,
        upper,
        (x - xs[lower]) / (xs[upper] - xs[lower]).max(f32::MIN_POSITIVE),
    )
}

/// The PSF table's `[rms_tangential, rms_sagittal, centroid_shift, edge]` (mm)
/// at normalized field `r` and defocus `dz` (mm), bilinear over the records
/// that hold rays (a zero record is a field the lens cannot image).
fn psf_at(psf: &PsfTable, r: f32, dz: f32) -> [f32; 4] {
    let (i0, i1, ti) = bracket(&psf.field_r, r);
    let (k0, k1, tk) = bracket(&psf.defocus_mm, dz);
    let mut sum = [0.0f32; 4];
    let mut weight = 0.0f32;
    for (i, wi) in [(i0, 1.0 - ti), (i1, ti)] {
        for (k, wk) in [(k0, 1.0 - tk), (k1, tk)] {
            let w = wi * wk;
            let rec = psf.record(i, k);
            if w <= 0.0 || rec[4] <= 0.0 {
                continue;
            }
            for (s, v) in sum.iter_mut().zip(&rec[1..5]) {
                *s += w * v;
            }
            weight += w;
        }
    }
    if weight <= 0.0 {
        return [0.0; 4];
    }
    sum.map(|v| v / weight)
}

/// Splits a spot into a Gaussian core and a comet tail and returns
/// `[sigma_r, sigma_t, signed share, tail length]`, mm.
///
/// The tail has the density `(1 - t)^2` along the radius over its length `L`,
/// the shape [`ab_coma`](crate::element_kind::COMA) draws: mean `L / 4`,
/// second moment `L^2 / 6`. The spot's far edge is `L`, its centroid shift
/// `c = share L / 4` and its radial variance `(1 - share) sigma^2 + share
/// L^2 / 6 - c^2`, which fixes the core's sigma so the tail is not counted
/// twice.
fn split_spot(rms_r: f32, rms_t: f32, centroid: f32, edge: f32) -> [f32; 4] {
    let length = edge.max(0.0);
    // A spot of rounding-error size has no tail, whatever the ratio of its
    // centroid to its edge.
    let share = if length > PSF_EPSILON_MM {
        (4.0 * centroid.abs() / length).min(0.9)
    } else {
        0.0
    };
    let c = share * length / 4.0;
    let core = ((rms_r * rms_r - share * length * length / 6.0 + c * c) / (1.0 - share))
        .max(0.0)
        .sqrt();
    [core, rms_t, share.copysign(centroid), length]
}

/// Words of every bake in `bakes`, in the layout described in the module doc.
pub(crate) fn pack(bakes: &[&LensBake]) -> Vec<u32> {
    let count = bakes.len();
    let mut words = vec![0u32; 1 + count + count * word::LEN];
    words[0] = count as u32;
    for (layer, bake) in bakes.iter().enumerate() {
        let header = 1 + count + layer * word::LEN;
        words[1 + layer] = header as u32;
        let mut put = |slot: usize, value: u32| words[header + slot] = value;
        let ray = &bake.ray_map;
        let vig = &bake.vignetting;
        let fp = &bake.footprints;
        let focus = &bake.focus;

        // The effective focal length is the small-field `y / tan` of the
        // reference wavelength; the distortion plane is `y / (s tan) - 1`
        // (infinity focus; at a finite focus it is the scaled equivalent).
        let mid = ray.n_field / 2;
        let y = ray.sensor_half_diagonal_mm * mid as f32 / (ray.n_field - 1).max(1) as f32;
        let tan = ray.tan_theta(0, mid);
        let efl = if tan > 0.0 {
            y / (tan * (1.0 + ray.distortion(0, mid)))
        } else {
            0.0
        };
        let efl = if efl.is_finite() && efl > 0.0 {
            efl
        } else {
            50.0
        };
        let focal_m = efl * 1.0e-3;

        let chromatic = (1..ray.n_field).any(|i| {
            let reference = ray.tan_theta(0, i).max(f32::MIN_POSITIVE);
            (1..ray.n_wave).any(|w| {
                (ray.tan_theta(w, i) - ray.tan_theta(0, i)).abs() / reference > CHROMATIC_EPSILON
            })
        });

        // Axial focus per channel: the bake's best-focus shifts weighted with
        // the sensor's response. A focus `dz` behind the sensor is the sensor
        // sitting in front of the image, which a nearer subject would cure:
        // object vergence changes by `-dz / f^2`.
        let mut channel = [0.0f32; 3];
        let mut total = [0.0f32; 3];
        for (k, &lambda) in focus.wavelengths_nm.iter().enumerate() {
            let dz = Some(focus.axial_real_mm[k])
                .filter(|v| v.is_finite())
                .unwrap_or(focus.axial_paraxial_mm[k]);
            for (c, w) in sensor_weights(lambda).into_iter().enumerate() {
                channel[c] += w * dz;
                total[c] += w;
            }
        }
        let loca = [0, 1, 2].map(|c| {
            if total[c] > 0.0 && channel[c].is_finite() {
                -channel[c] / total[c] * 1.0e-3 / (focal_m * focal_m)
            } else {
                0.0
            }
        });

        // One record per analysis field, from the point spread at that
        // field's own best focus (its field curvature belongs to the focus
        // field, not to the point spread).
        let n_fields = focus.n_field;
        let mut analysis = Vec::with_capacity(n_fields * ANALYSIS_STRIDE);
        let mut has_psf = false;
        let axis_best = focus.best_mm.first().copied().unwrap_or(0.0);
        for i in 0..n_fields {
            let r = focus.field_r[i];
            let spot = psf_at(&bake.psf, r, focus.best_mm[i]);
            let [core, sagittal, share, length] = split_spot(spot[0], spot[1], spot[2], spot[3]);
            has_psf |= core > PSF_EPSILON_MM || sagittal > PSF_EPSILON_MM || share != 0.0;
            let curvature = -(focus.best_mm[i] - axis_best) * 1.0e-3 / (focal_m * focal_m);
            push_f32(&mut analysis, [r, curvature, core, sagittal, share, length]);
        }

        let mut flags = 0;
        if chromatic {
            flags |= FLAG_CHROMATIC;
        }
        if has_psf {
            flags |= FLAG_PSF;
        }
        put(word::EFL, efl.to_bits());
        put(
            word::F_MIN,
            vig.f_numbers
                .first()
                .copied()
                .unwrap_or(fp.f_number)
                .to_bits(),
        );
        put(word::HALF_DIAGONAL, ray.sensor_half_diagonal_mm.to_bits());
        put(word::FLAGS, flags);
        put(word::FOOTPRINT_RESOLUTION, fp.resolution as u32);
        put(word::FOOTPRINT_FIELDS, fp.n_fields as u32);
        put(word::FOOTPRINT_F_NUMBER, fp.f_number.to_bits());
        for (c, v) in loca.into_iter().enumerate() {
            put(word::FOCUS_RED + c, v.to_bits());
        }
        put(word::RAY_FIELDS, ray.n_field as u32);
        put(word::RAY_WAVES, ray.n_wave as u32);
        put(word::VIG_FIELDS, vig.n_field as u32);
        put(word::VIG_F_NUMBERS, vig.n_fnum as u32);
        put(word::ANALYSIS_FIELDS, n_fields as u32);

        // Tables follow the headers; their offsets go back into the header.
        let table = |words: &mut Vec<u32>, slot: usize, values: &[f32]| {
            let at = words.len() as u32;
            words[header + slot] = at;
            push_f32(words, values.iter().copied());
        };
        table(&mut words, word::RAY_WAVELENGTHS, &ray.wavelengths_nm);
        // Plane 0 of the ray map is `tan_theta`; the distortion plane follows
        // from it and the effective focal length.
        table(
            &mut words,
            word::RAY_TAN,
            &ray.data[..ray.n_wave * ray.n_field],
        );
        table(&mut words, word::VIG_F_NUMBER_LIST, &vig.f_numbers);
        table(&mut words, word::VIG_DATA, &vig.data);
        table(&mut words, word::FOOTPRINT_FIELD_LIST, &fp.field_r);
        table(&mut words, word::FOOTPRINT_DATA, &fp.data);
        let at = words.len() as u32;
        words[header + word::ANALYSIS_DATA] = at;
        words.extend(analysis);
    }
    words
}

/// Baked lens prescriptions for [`crate::LensPass`], one layer per
/// [`LensBake`]. Register [`Self::buffer`] under [`PRESCRIPTIONS`] (or hand it
/// to [`crate::LensInputs::prescriptions`]); a
/// [`PRESCRIPTION`](crate::element_kind::PRESCRIPTION) element names a layer in
/// `params[0]`. The layers are immutable: build a new bank to change them.
pub struct PrescriptionBank {
    buffer: wgpu::Buffer,
    efl_mm: Vec<f32>,
    f_min: Vec<f32>,
    rays: Vec<PrescriptionRays>,
}

impl PrescriptionBank {
    /// Uploads `bakes`; layer `i` is `bakes[i]`.
    pub fn new(device: &wgpu::Device, bakes: &[&LensBake]) -> Self {
        use wgpu::util::DeviceExt;
        let mut words = pack(bakes);
        // A storage binding is never empty.
        words.resize(words.len().max(4), 0);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Lens prescriptions"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let header =
            |layer: usize, slot: usize| f32::from_bits(words[words[1 + layer] as usize + slot]);
        Self {
            buffer,
            efl_mm: (0..bakes.len()).map(|l| header(l, word::EFL)).collect(),
            f_min: (0..bakes.len()).map(|l| header(l, word::F_MIN)).collect(),
            rays: (0..bakes.len())
                .map(|l| PrescriptionRays::from_words(&words, l))
                .collect(),
        }
    }

    /// The storage buffer for [`PRESCRIPTIONS`].
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Layers in the bank.
    pub fn len(&self) -> usize {
        self.efl_mm.len()
    }

    pub fn is_empty(&self) -> bool {
        self.efl_mm.is_empty()
    }

    /// Effective focal length of `layer` in millimetres: what a full-frame
    /// element sets the view's focal length to, so a camera should render with
    /// the field of view of this focal length on the lens's sensor.
    pub fn efl_mm(&self, layer: usize) -> f32 {
        self.efl_mm[layer]
    }

    /// The widest aperture `layer` was baked at: a full-frame element keeps the
    /// view's f-number at or above it.
    pub fn min_f_number(&self, layer: usize) -> f32 {
        self.f_min[layer]
    }

    /// The CPU copy of the layers' ray maps, for [`crate::ray_map::RayMap::with_rays`].
    pub fn rays(&self) -> &[PrescriptionRays] {
        &self.rays
    }
}

/// The ray map of one bank layer, kept on the CPU: what `lens.wgsl`'s `rx_map`
/// reads, so [`crate::ray_map::RayMap`] can reach the same scene positions
/// the shader does (chiefly for the overscan a camera renders).
#[derive(Clone, Debug, PartialEq)]
pub struct PrescriptionRays {
    efl_mm: f32,
    f_min: f32,
    half_diagonal_mm: f32,
    n_field: usize,
    wavelengths_nm: Vec<f32>,
    /// `tan_theta[wave * n_field + field]`.
    tan_theta: Vec<f32>,
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a * (1.0 - t) + b * t
}

impl PrescriptionRays {
    /// Reads layer `layer` of packed `words` (see [`pack`]).
    fn from_words(words: &[u32], layer: usize) -> Self {
        let h = words[1 + layer] as usize;
        let f = |slot: usize| f32::from_bits(words[h + slot]);
        let table = |slot: usize, len: usize| {
            let at = words[h + slot] as usize;
            words[at..at + len]
                .iter()
                .map(|&w| f32::from_bits(w))
                .collect::<Vec<_>>()
        };
        let n_field = words[h + word::RAY_FIELDS] as usize;
        let n_wave = words[h + word::RAY_WAVES] as usize;
        Self {
            efl_mm: f(word::EFL),
            f_min: f(word::F_MIN),
            half_diagonal_mm: f(word::HALF_DIAGONAL),
            n_field,
            wavelengths_nm: table(word::RAY_WAVELENGTHS, n_wave),
            tan_theta: table(word::RAY_TAN, n_wave * n_field),
        }
    }

    /// Effective focal length (mm), see [`PrescriptionBank::efl_mm`].
    pub fn efl_mm(&self) -> f32 {
        self.efl_mm
    }

    /// The widest aperture the layer was baked at, see
    /// [`PrescriptionBank::min_f_number`].
    pub fn min_f_number(&self) -> f32 {
        self.f_min
    }

    /// `rx_bracket`: the lower index and the fraction to the next entry of an
    /// ascending list, clamped to the ends.
    fn bracket(list: &[f32], x: f32) -> (usize, f32) {
        let n = list.len();
        let mut lower = 0;
        for (k, &entry) in list.iter().enumerate().take(n.saturating_sub(1)).skip(1) {
            if entry <= x {
                lower = k;
            }
        }
        if n < 2 {
            return (0, 0.0);
        }
        (
            lower,
            ((x - list[lower]) / (list[lower + 1] - list[lower]).max(1.0e-6)).clamp(0.0, 1.0),
        )
    }

    /// `rx_tan`: `tan(theta)` of the scene ray the normalised sensor radius `r`
    /// looks at, for the wavelength position `t` (-1 blue .. 1 red); linear in
    /// the field, continued with the corner's slope beyond it.
    fn tan(&self, r: f32, t: f32) -> f32 {
        let nf = self.n_field;
        let p = r.max(0.0) * (nf - 1) as f32;
        let i0 = (p.floor() as usize).min(nf - 2);
        let f = p - i0 as f32;
        let (k0, wf) = Self::bracket(&self.wavelengths_nm, 550.0 + 150.0 * t);
        let row = |k: usize| {
            let a = k * nf + i0;
            mix(self.tan_theta[a], self.tan_theta[a + 1], f)
        };
        let mut tan_theta = row(k0);
        if wf > 0.0 {
            tan_theta = mix(tan_theta, row(k0 + 1), wf);
        }
        tan_theta
    }

    /// `rx_map`: the scene position, in focal-length units, the sensor point
    /// `x` looks at when the view's focal length is `focal_mm`.
    pub(crate) fn map(&self, x: [f32; 2], t: f32, focal_mm: f32) -> [f32; 2] {
        let len = (x[0] * x[0] + x[1] * x[1]).sqrt();
        if len < 1.0e-6 {
            let k = focal_mm / self.efl_mm;
            return [x[0] * k, x[1] * k];
        }
        let k = self.tan(len * focal_mm / self.half_diagonal_mm, t) / len;
        [x[0] * k, x[1] * k]
    }
}

/// The CPU ray maps of `bakes`, one per layer, as a bank of them would hold.
pub fn pack_rays(bakes: &[&LensBake]) -> Vec<PrescriptionRays> {
    let words = pack(bakes);
    (0..bakes.len())
        .map(|layer| PrescriptionRays::from_words(&words, layer))
        .collect()
}
