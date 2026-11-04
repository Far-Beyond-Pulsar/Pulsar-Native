// The atmosphere's cloud layer (`CloudLayer`): a band of altitudes over the
// planet's surface where the weather gathers clouds, billows shape them and
// wisps erode their edges (after Schneider's Nubis, Horizon Zero Dawn), lit
// by the sun through the air with multiple scattering (Wrenninge's octaves,
// as in Hillaire 2016), and a sheet of cirrus above it. Needs the
// `atmosphere` snippet and bindings named `cloud_planet_weather`
// (texture_cube<f32>, `cloud_planet_weather.wgsl`'s map), `cloud_weather`,
// `cloud_shape`, `cloud_detail` (texture_3d<f32>, the noise
// `cloud_noise.wgsl` builds) and `cloud_noise_sampler` (linear, repeating,
// mipmapped). Positions are kilometres relative to the planet's centre,
// like the atmosphere's.
//
// The cover is the weather at four scales: the planet's (its circulation
// and synoptic systems, the map), the mesoscale cells, the cumulus groups,
// then the billows. Each scale below the planet's is a tiling noise volume
// whose mips keep the spread of the values under them (a mean and a
// standard deviation per channel), so the share of sky the weather covers
// holds at any distance: a footprint wider than the noise's features covers
// the probability of a cloud there instead of a guess, from the ground to
// orbit.
//
// Voxel clouds (`CloudRepresentation::Voxels`) read the same density at
// the centres of cubic cells on a lattice fixed to the planet, each cell
// filled or empty by it: everything that reads the density (the trace, the
// shadow map, the sun's visibility, the fog's shafts) sees the same blocks.

// Texels along each side of the noise volumes.
const CLOUD_WEATHER_TEXELS: f32 = 64.0;
const CLOUD_SHAPE_TEXELS: f32 = 128.0;
const CLOUD_DETAIL_TEXELS: f32 = 32.0;
// How far the planet's weather moves the cover: logits per unit of its
// anomaly.
const CLOUD_WEATHER_CONTRAST: f32 = 2.0;
// The regional share's logit stays within this: even the thickest deck
// keeps lanes between its cells, the clearest high a few clouds.
const CLOUD_SHARE_LIMIT: f32 = 2.6;
// How far the weather's regimes move the form away from the layer's.
const CLOUD_REGIME_SPAN: f32 = 0.9;
// The mesoscale's share of the local field (the groups' is the rest), on a
// planet a few of its cells round: on one smaller than its cells the
// mesoscale fades out (the whole planet would sit in one cell, its cover
// shifted everywhere alike). And the cells' share of the mesoscale (the
// rest is fractal noise).
const CLOUD_MESO_SHARE: f32 = 0.6;
const CLOUD_CELLS_SHARE: f32 = 0.65;
// The cover's edge, in standard deviations of the local field.
const CLOUD_EDGE: f32 = 0.55;
// The cirrus sheet's optical depth at full cover, straight through, and
// how far its streaks stretch along the jets.
const CLOUD_CIRRUS_DEPTH: f32 = 0.7;
const CLOUD_CIRRUS_STRETCH: f32 = 6.0;

// The layer's parameters as the density reads them, gathered once per
// invocation (the frame is too large to pass to every density sample).
struct CloudLayerParams {
    // Radii of the planet, the layer's bottom and top (km); 1 / thickness.
    planet_radius: f32,
    inner: f32,
    outer: f32,
    inv_thickness: f32,
    // The mean coverage's logit, and whether the layer has any cover.
    coverage_logit: f32,
    covered: bool,
    kind: f32,
    extinction: f32,
    erosion: f32,
    inv_groups: f32,
    inv_meso: f32,
    inv_shape: f32,
    inv_detail: f32,
    // The mesoscale's and the groups' shares of the local field.
    meso_share: f32,
    groups_share: f32,
    // Noise texels per kilometre of each scale, for mips from footprints.
    groups_texels: f32,
    meso_texels: f32,
    shape_texels: f32,
    // The wisps' mip relative to the billows'.
    detail_lod_bias: f32,
    groups_offset: vec3<f32>,
    meso_offset: vec3<f32>,
    meso_fine_offset: vec3<f32>,
    shape_offset: vec3<f32>,
    detail_offset: vec3<f32>,
    albedo: vec3<f32>,
    // The local field's channels over the whole volume: mean and standard
    // deviation of the fractal noise (xy) and of the cells (zw).
    field: vec4<f32>,
    // The cirrus: its sheet's radius, amount and the jets' axis.
    cirrus_radius: f32,
    cirrus: f32,
    pole: vec3<f32>,
    // Voxel cells' side (km; 0: smooth) and the density they fill from.
    voxel: f32,
    fill: f32,
}

fn cloud_logit(p: f32) -> f32 {
    let q = clamp(p, 1e-3, 1.0 - 1e-3);
    return log(q / (1.0 - q));
}

fn cloud_logistic(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-clamp(x, -30.0, 30.0)));
}

fn cloud_layer_params(frame: AtmosphereFrame) -> CloudLayerParams {
    let p = frame.params;
    var c: CloudLayerParams;
    c.planet_radius = p.bottom_radius;
    c.inner = p.bottom_radius + p.cloud_bottom;
    c.outer = p.bottom_radius + max(p.cloud_top, p.cloud_bottom + 1e-3);
    c.inv_thickness = 1.0 / (c.outer - c.inner);
    c.coverage_logit = cloud_logit(p.cloud_coverage);
    c.covered = p.cloud_coverage > 0.0;
    c.kind = clamp(p.cloud_type, 0.0, 1.0);
    c.extinction = max(p.cloud_extinction, 0.0);
    c.erosion = clamp(p.cloud_erosion, 0.0, 1.0);
    c.inv_groups = 1.0 / max(p.cloud_weather_scale, 1e-3);
    c.inv_meso = 1.0 / max(p.cloud_mesoscale_scale, 1e-3);
    c.inv_shape = 1.0 / max(p.cloud_shape_scale, 1e-3);
    c.inv_detail = 1.0 / max(p.cloud_detail_scale, 1e-3);
    c.meso_share = CLOUD_MESO_SHARE * smoothstep(1.0, 4.0, p.bottom_radius * c.inv_meso);
    c.groups_share = 1.0 - c.meso_share;
    c.groups_texels = c.inv_groups * CLOUD_WEATHER_TEXELS;
    c.meso_texels = c.inv_meso * CLOUD_WEATHER_TEXELS;
    c.shape_texels = c.inv_shape * CLOUD_SHAPE_TEXELS;
    c.detail_lod_bias = log2((max(p.cloud_shape_scale, 1e-3) / CLOUD_SHAPE_TEXELS)
        / (max(p.cloud_detail_scale, 1e-3) / CLOUD_DETAIL_TEXELS));
    c.groups_offset = frame.cloud_groups_offset.xyz;
    c.meso_offset = frame.cloud_meso_offset.xyz;
    c.meso_fine_offset = frame.cloud_meso_fine_offset.xyz;
    c.shape_offset = frame.cloud_shape_offset.xyz;
    c.detail_offset = frame.cloud_detail_offset.xyz;
    c.albedo = clamp(p.cloud_albedo, vec3<f32>(0.0), vec3<f32>(1.0));
    // The volume's last mip: one texel, its whole spread.
    c.field = textureLoad(cloud_weather, vec3<i32>(0), i32(textureNumLevels(cloud_weather)) - 1);
    c.cirrus_radius = p.bottom_radius + max(p.cloud_cirrus_height, p.cloud_top + 0.1);
    c.cirrus = clamp(p.cloud_cirrus, 0.0, 1.0);
    let pole = p.cloud_pole;
    c.pole = select(vec3<f32>(0.0, 1.0, 0.0), normalize(pole), dot(pole, pole) > 1e-8);
    c.voxel = max(p.cloud_voxel_size, 0.0);
    c.fill = clamp(p.cloud_voxel_fill, 0.0, 1.0);
    return c;
}

fn cloud_remap(x: f32, lo: f32, hi: f32) -> f32 {
    return (x - lo) / max(hi - lo, 1e-5);
}

// Density over the layer's height `h` (0 bottom, 1 top) for a form `kind`:
// stratus is a thin sheet low in the layer, cumulus rises through it, flat
// at the base and rounded at the crown.
fn cloud_height_profile(h: f32, kind: f32) -> f32 {
    let top = mix(0.35, 1.0, kind);
    let base = smoothstep(0.0, mix(0.08, 0.14, kind), h);
    let crown = 1.0 - smoothstep(top * mix(0.55, 0.3, kind), top, h);
    return base * crown;
}

// The mip of a noise of `texels_per_km` under a footprint of `footprint` km.
fn cloud_lod(footprint: f32, texels_per_km: f32) -> f32 {
    return max(log2(max(footprint * texels_per_km, 1e-6)), 0.0);
}

// The planet's weather over direction `anchor` (`cloud_planet_weather.wgsl`).
fn cloud_planet_weather_at(anchor: vec3<f32>) -> vec4<f32> {
    return textureSampleLevel(cloud_planet_weather, cloud_noise_sampler, anchor, 0.0);
}

// The mesoscale's mix for cells as open as `open`: x the cells' weight
// against the fractal noise, y 1 for open cells (cumulus rings round clear
// cells, the cells' far edges), 0 for closed (cloudy cells parted by clear
// lanes, their centres). Half-open cells are neither: the noise alone.
fn cloud_cells(open: f32) -> vec2<f32> {
    let k = 2.0 * open - 1.0;
    return vec2<f32>(CLOUD_CELLS_SHARE * abs(k), select(0.0, 1.0, k > 0.0));
}

// The mesoscale field of channels `t` (means and spreads: fractal noise,
// cells) under `cells` (`cloud_cells`): x its value, y its variance.
fn cloud_meso_field(t: vec4<f32>, cells: vec2<f32>) -> vec2<f32> {
    let cell = mix(1.0 - t.b, t.b, cells.y);
    let w = cells.x;
    return vec2<f32>(mix(t.r, cell, w), (1.0 - w) * (1.0 - w) * t.g * t.g + w * w * t.a * t.a);
}

// How much cloud the weather allows over `anchor` (the point's direction at
// the layer's bottom, km) under a footprint of `footprint` km: x the
// probability of a cloud there (0 clear sky, 1 the billows' full extent),
// y the local form.
fn cloud_weather_at(c: CloudLayerParams, anchor: vec3<f32>, footprint: f32) -> vec2<f32> {
    if !c.covered { return vec2<f32>(0.0, c.kind); }
    let planet = cloud_planet_weather_at(anchor);
    let cells = cloud_cells(planet.a);
    let meso_lod = cloud_lod(footprint, c.meso_texels);
    // Two mesoscale lookups at incommensurate sizes, turned apart: their
    // sum never repeats.
    let ma = textureSampleLevel(cloud_weather, cloud_noise_sampler, CLOUD_MESO_TURN_A * (anchor * c.inv_meso) + c.meso_offset, meso_lod);
    let mb = textureSampleLevel(cloud_weather, cloud_noise_sampler,
        CLOUD_MESO_TURN_B * (anchor * (c.inv_meso * CLOUD_MESO_RATIO)) + c.meso_fine_offset,
        meso_lod + log2(CLOUD_MESO_RATIO));
    let a = cloud_meso_field(ma, cells);
    let b = cloud_meso_field(mb, cells);
    let groups = textureSampleLevel(cloud_weather, cloud_noise_sampler, anchor * c.inv_groups + c.groups_offset,
        cloud_lod(footprint, c.groups_texels));
    let meso = 0.6 * a.x + 0.4 * b.x;
    let q = c.meso_share * meso + c.groups_share * groups.r;
    let lost = c.meso_share * c.meso_share * (0.36 * a.y + 0.16 * b.y)
        + c.groups_share * c.groups_share * groups.g * groups.g;
    // The field's distribution over the whole volume, as `q`'s (the two
    // lookups' spreads add as independent).
    let f = c.field;
    let whole = cloud_meso_field(f, cells);
    let mean = c.meso_share * whole.x + c.groups_share * f.x;
    let spread = c.meso_share * c.meso_share * 0.52 * whole.y + c.groups_share * c.groups_share * f.y * f.y;
    let sigma = sqrt(max(spread, 1e-8));
    // The share of sky the weather covers here, and the chance the local
    // field (its mean under the footprint, give or take what the footprint
    // hides) exceeds that share's quantile: the cover's logit.
    let share = clamp(c.coverage_logit + CLOUD_WEATHER_CONTRAST * planet.r, -CLOUD_SHARE_LIMIT, CLOUD_SHARE_LIMIT);
    let s = sqrt(lost + CLOUD_EDGE * CLOUD_EDGE * spread);
    let cover = cloud_logistic(1.6 * (q - mean) / s + (sigma / s) * share);
    let kind = clamp(c.kind + (planet.g - 0.5) * CLOUD_REGIME_SPAN + (groups.r - f.x) * 0.6, 0.0, 1.0);
    return vec2<f32>(cover, kind);
}

// E[max(Y, 0)] for Y about `mean` with standard deviation `sigma` (a
// logistic's, close to a normal's).
fn cloud_soft_positive(mean: f32, sigma: f32) -> f32 {
    // Resolved (no spread under the footprint): the clamp itself.
    if sigma < 1e-3 { return max(mean, 0.0); }
    let s = sigma * 0.5513;
    let z = mean / s;
    return max(mean, 0.0) + s * log(1.0 + exp(-abs(z)));
}

// The density that extinguishes like mean density `d` gathered in billows
// filling `inside` of a footprint of `footprint` km: once the billows are
// smaller than the footprint, light passes between them as through clumps
// (each about a quarter of the billows' size), not through their mean
// smeared evenly (which would be opaque wherever any cloud is). From afar
// a field of cumulus then hides about the share of the ground it covers.
// How far billows are smaller than a footprint of `footprint` km: 0 resolved,
// 1 well below it.
fn cloud_unresolved(c: CloudLayerParams, footprint: f32) -> f32 {
    return smoothstep(0.5, 2.0, footprint * c.inv_shape * 4.0);
}

fn cloud_clumped(c: CloudLayerParams, d: f32, inside: f32, footprint: f32) -> f32 {
    let clump = 0.25 / c.inv_shape;
    let unresolved = cloud_unresolved(c, footprint);
    if unresolved <= 0.0 || inside <= 1e-4 || c.extinction <= 0.0 { return d; }
    let within = d / inside * c.extinction;
    let effective = inside * (1.0 - exp(-within * clump)) / (clump * c.extinction);
    return mix(d, effective, unresolved);
}

// Cloud density (0..1, times `extinction` per km) at planet-relative point
// `x`, at height `h` in the layer (0..1), under `weather`
// (`cloud_weather_at`), for a footprint of `footprint` km; the wisps only
// when `detail`. The billows' carving is its mean under the footprint.
// The centre of the voxel cell holding planet-relative point `x`.
fn cloud_cell(c: CloudLayerParams, x: vec3<f32>) -> vec3<f32> {
    return (floor(x / c.voxel) + 0.5) * c.voxel;
}

// Where the density of planet-relative point `x` is read: its voxel cell's
// centre, or itself.
fn cloud_point(c: CloudLayerParams, x: vec3<f32>) -> vec3<f32> {
    return select(x, cloud_cell(c, x), c.voxel > 0.0);
}

fn cloud_density_under(c: CloudLayerParams, x: vec3<f32>, h: f32, weather: vec2<f32>, footprint: f32, detail: bool) -> f32 {
    if c.voxel <= 0.0 { return cloud_smooth_density(c, x, h, weather, footprint, detail); }
    // Voxels: the smooth density at the cell's centre, filled or empty
    // where the cells are large enough to see, blending into it where they
    // are not.
    let cell = cloud_cell(c, x);
    let d = cloud_smooth_density(c, cell, cloud_height(c, cell), weather, max(footprint, c.voxel), detail);
    let blocky = 1.0 - smoothstep(1.0, 4.0, footprint / c.voxel);
    return mix(d, smoothstep(c.fill - 0.02, c.fill + 0.02, d), blocky);
}

fn cloud_smooth_density(c: CloudLayerParams, x: vec3<f32>, h: f32, weather: vec2<f32>, footprint: f32, detail: bool) -> f32 {
    if h <= 0.0 || h >= 1.0 || weather.x <= 1e-4 { return 0.0; }
    let profile = cloud_height_profile(h, weather.y);
    if profile <= 0.0 { return 0.0; }
    let lod = cloud_lod(footprint, c.shape_texels);
    let s = textureSampleLevel(cloud_shape, cloud_noise_sampler, x * c.inv_shape + c.shape_offset, lod);
    // Cover carves the billows (thin cover keeps only their cores) and
    // scales what is left: clamp(billows * profile + cover - 1, 0, cover).
    let mean = s.r * profile + weather.x - 1.0;
    // The spread counts only where the billows are smaller than the
    // footprint: where they are resolved, the mip's spread is noise the
    // next steps resolve, and its tail would lay a haze over the clear sky
    // between clouds (a dim shadow everywhere from a shadow map's texels).
    let sigma = s.g * profile * cloud_unresolved(c, footprint);
    let d = cloud_soft_positive(mean, sigma) - cloud_soft_positive(mean - weather.x, sigma);
    if d <= 1e-4 { return 0.0; }
    // The share of the footprint inside a billow.
    let inside = cloud_logistic(mean / max(sigma * 0.5513, 1e-5));
    if !detail { return cloud_clumped(c, d, inside, footprint); }
    let detail_lod = max(lod + c.detail_lod_bias, 0.0);
    let n = textureSampleLevel(cloud_detail, cloud_noise_sampler, x * c.inv_detail + c.detail_offset, detail_lod);
    let wisps = n.r * 0.625 + n.g * 0.25 + n.b * 0.125;
    // Wispy at the base, billowing towards the crown.
    let erode = mix(wisps, 1.0 - wisps, clamp(h * 4.0, 0.0, 1.0)) * c.erosion;
    let eroded = clamp(cloud_remap(d, erode, 1.0), 0.0, 1.0);
    // Wisps finer than the footprint thin the clouds by their mean instead:
    // a threshold on a footprint's mean density would erase it.
    return cloud_clumped(c, mix(eroded, d * (1.0 - 0.5 * c.erosion), smoothstep(4.0, 6.0, detail_lod)), inside, footprint);
}

// The layer's height (0..1) of planet-relative point `x`.
fn cloud_height(c: CloudLayerParams, x: vec3<f32>) -> f32 {
    return (length(x) - c.inner) * c.inv_thickness;
}

// Cloud density at `x` under its own weather (a voxel cell's, its
// centre's). x: the density, y: the height in the layer, zw: the weather
// there (`cloud_weather_at`; zero outside).
fn cloud_density(c: CloudLayerParams, point: vec3<f32>, footprint: f32, detail: bool) -> vec4<f32> {
    let x = cloud_point(c, point);
    let r = length(x);
    let h = (r - c.inner) * c.inv_thickness;
    if h <= 0.0 || h >= 1.0 { return vec4<f32>(0.0, h, 0.0, 0.0); }
    let weather = cloud_weather_at(c, x * (c.inner / r), footprint);
    return vec4<f32>(cloud_density_under(c, x, h, weather, footprint, detail), h, weather);
}

// The cirrus sheet's optical depth straight through it over planet-relative
// point `x` (on the sheet), for a footprint of `footprint` km: the
// weather's cirrus, streaked along the jets (east, round the pole).
fn cirrus_depth(c: CloudLayerParams, x: vec3<f32>, footprint: f32) -> f32 {
    if c.cirrus <= 0.0 { return 0.0; }
    let up = normalize(x);
    let amount = cloud_planet_weather_at(up).b * c.cirrus;
    if amount <= 1e-3 { return 0.0; }
    let east = cross(c.pole, up);
    let along = select(vec3<f32>(1.0, 0.0, 0.0), east / sqrt(max(dot(east, east), 1e-12)), dot(east, east) > 1e-8);
    // Compressed along the jet, the noise's features stretch along it.
    let streaked = x - along * (dot(x, along) * (1.0 - 1.0 / CLOUD_CIRRUS_STRETCH));
    let sheets = textureSampleLevel(cloud_weather, cloud_noise_sampler, streaked * (c.inv_meso * 2.0) + c.meso_offset * 2.0,
        cloud_lod(footprint, c.meso_texels * 2.0));
    let streaks = textureSampleLevel(cloud_detail, cloud_noise_sampler, streaked * (c.inv_shape * 0.5),
        cloud_lod(footprint, c.inv_shape * 0.5 * CLOUD_DETAIL_TEXELS));
    let field = sheets.r * 0.6 + (streaks.r * 0.625 + streaks.g * 0.375) * 0.4;
    // Thin veils where the weather brings a little, streaked sheets where
    // it brings more.
    return smoothstep(0.3, 0.8, field + (amount - 0.5) * 0.6) * amount * CLOUD_CIRRUS_DEPTH;
}

// The span of a ray from `o` along `d` (planet-relative, km) inside the
// layer: x start, y end (y <= x: it misses). From below, the span up
// through the layer (none if the ground comes first); from inside, up to
// where the ray leaves; from above, from the top down to the bottom or out.
// Only the first span: a ray that passes under the layer and rises into it
// again beyond the horizon stops at its bottom.
fn cloud_layer_segment(c: CloudLayerParams, o: vec3<f32>, d: vec3<f32>) -> vec2<f32> {
    let outer = ray_sphere(o, d, c.outer);
    if outer.y <= 0.0 { return vec2<f32>(0.0, -1.0); }
    let inner = ray_sphere(o, d, c.inner);
    let r = length(o);
    if r < c.inner {
        let ground = ray_sphere(o, d, c.planet_radius);
        if ground.x > 0.0 { return vec2<f32>(0.0, -1.0); }
        return vec2<f32>(inner.y, outer.y);
    }
    let start = select(0.0, outer.x, r > c.outer);
    let end = select(outer.y, inner.x, inner.x > 0.0);
    return vec2<f32>(start, end);
}

// Where a ray from `o` along `d` crosses the cirrus sheet first (km), or
// a negative distance if it does not (or the ground comes first).
fn cirrus_crossing(c: CloudLayerParams, o: vec3<f32>, d: vec3<f32>) -> f32 {
    let sheet = ray_sphere(o, d, c.cirrus_radius);
    if sheet.y <= 0.0 { return -1.0; }
    if length(o) < c.cirrus_radius {
        let ground = ray_sphere(o, d, c.planet_radius);
        if ground.x > 0.0 { return -1.0; }
        return sheet.y;
    }
    return sheet.x;
}

// Henyey-Greenstein.
fn cloud_hg(cos_theta: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * ATMOSPHERE_PI * pow(max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4), 1.5));
}

// Light scattered towards the eye at a point whose optical depth towards
// the sun is `od`, per unit of scattering and of sun: Wrenninge's multiple
// scattering octaves (each bounce weaker, less extinguished and less
// forward), over a phase of a strong forward lobe and a backward one (the
// silver lining and the darkened back of a cloud seen against the sun).
// Four octaves, each 0.6 of the last's light, 0.3 of its extinction: thick
// clouds keep most of a diffuse reflector's light (seen from above, their
// tops are brighter than the ground) at the price of thin ones scattering
// about twice their single scattering.
const CLOUD_OCTAVES: u32 = 4u;

fn cloud_sun_scattering(cos_theta: f32, od: f32) -> f32 {
    var total = 0.0;
    var a = 1.0;
    var b = 1.0;
    var c = 1.0;
    for (var k = 0u; k < CLOUD_OCTAVES; k++) {
        let phase = mix(cloud_hg(cos_theta, 0.8 * c), cloud_hg(cos_theta, -0.3 * c), 0.3);
        total += a * phase * exp(-od * b);
        a *= 0.6;
        b *= 0.3;
        c *= 0.5;
    }
    return total;
}
