// World constants and helpers shared by the engine and terrain programs:
// the grid mapping, layer quantization and material ids.
const HEIGHT_ONE: i32 = 1000; // height units (mm) per metre

struct World {
    grid: vec4<i32>,  // reference cells, layer thickness (mm), grid cells, level offset
    scale: vec4<u32>, // domain scale (Q24), volume inv, volume shift, half layer (Q16 domain units)
    sphere: vec4<u32>, // sphere domain: 1 / reference (inv, shift), domain radius; w: pad
}

// Engine material ids (terrain::material).
const M_AIR: u32 = 0u;
const M_GRASS: u32 = 1u;
const M_DIRT: u32 = 2u;
const M_STONE: u32 = 3u;
const M_SAND: u32 = 4u;
const M_SNOW: u32 = 5u;
const M_GRAVEL: u32 = 7u;
const M_SANDSTONE: u32 = 8u;
const M_DARK_STONE: u32 = 9u;
const M_CLAY: u32 = 12u;
const M_REGOLITH: u32 = 16u;
const M_MARE: u32 = 17u;
const M_EJECTA: u32 = 18u;
const M_BOULDER: u32 = 19u;
const M_BASALT: u32 = 20u;
const M_ANORTHOSITE: u32 = 21u;
// Ground material flag: a single-voxel fleck that blends into grass once
// its cell is about a pixel wide.
const M_SPECK: u32 = 0x100u;
const M_ID: u32 = 0xffu;

// Face bases, matching grid::FACE_BASIS.
fn face_axis(face: u32, axis: u32) -> vec3<i32> {
    switch face * 3u + axis {
        case 0u: { return vec3<i32>(1, 0, 0); }
        case 1u: { return vec3<i32>(0, 0, -1); }
        case 2u: { return vec3<i32>(0, 1, 0); }
        case 3u: { return vec3<i32>(-1, 0, 0); }
        case 4u: { return vec3<i32>(0, 0, 1); }
        case 5u: { return vec3<i32>(0, 1, 0); }
        case 6u: { return vec3<i32>(0, 1, 0); }
        case 7u: { return vec3<i32>(1, 0, 0); }
        case 8u: { return vec3<i32>(0, 0, -1); }
        case 9u: { return vec3<i32>(0, -1, 0); }
        case 10u: { return vec3<i32>(1, 0, 0); }
        case 11u: { return vec3<i32>(0, 0, 1); }
        case 12u: { return vec3<i32>(0, 0, 1); }
        case 13u: { return vec3<i32>(1, 0, 0); }
        case 14u: { return vec3<i32>(0, 1, 0); }
        case 15u: { return vec3<i32>(0, 0, -1); }
        case 16u: { return vec3<i32>(-1, 0, 0); }
        default: { return vec3<i32>(0, 1, 0); }
    }
}

fn mul_q24(a: u32, r: u32) -> u32 {
    let a1 = a >> 16u;
    let a0 = a & 0xffffu;
    let r1 = r >> 16u;
    let r0 = r & 0xffffu;
    return ((a1 * r1) << 8u) + ((a1 * r0 + a0 * r1) >> 8u) + ((a0 * r0) >> 24u);
}

// Centre of level cell `i` in half base cells.
fn center_half(i: i32, level: u32) -> i32 {
    return (i << (level + 1u)) + (1 << level);
}

// Level cell centre in half cells of the reference grid (grid independent).
// Plane domain point in domain units (1.25 cm) relative to the world origin
// (`grid::plane_domain_point`): scaled by magnitude, symmetric about it.
fn plane_scaled(x: i32, level: u32) -> i32 {
    let origin = world.grid.z / 2;
    let v = (x << (level + 1u)) + (1 << level) - (origin << 1u);
    let m = i32(mul_q24(u32(abs(v)), world.scale.x) << 2u);
    return select(m, -m, v < 0);
}

fn domain_point(face: u32, i: i32, j: i32, level: u32) -> vec3<i32> {
    if is_plane() {
        return vec3<i32>(plane_scaled(i, level), 0, -plane_scaled(j, level));
    }
    let reference = world.grid.x;
    let scale = world.scale.x;
    let half = 1u << level;
    let u = i32(mul_q24((bitcast<u32>(i) << (level + 1u)) + half, scale)) - reference;
    let v = i32(mul_q24((bitcast<u32>(j) << (level + 1u)) + half, scale)) - reference;
    return sphere_point(face_axis(face, 0u) * reference + face_axis(face, 1u) * u + face_axis(face, 2u) * v);
}

// tan(pi/4 x) in Q30, exact at 0 and +-1 (`grid::tan_quarter`).
fn tan_quarter(x: i32) -> i32 {
    let ax = min(u32(abs(x)), Q30);
    let x2 = mul_shr(ax, ax, 30u);
    let poly = 230426967u + mul_shr(53687091u, x2, 30u);
    let m = i32(ax - mul_shr(ax, mul_shr(Q30 - x2, poly, 30u), 30u));
    return select(m, -m, x < 0);
}

// 1 / sqrt(d) in Q30 for d in [1, 3] (`grid::rsqrt_q30`).
fn rsqrt_q30(d: u32) -> u32 {
    var y = 1288490189u - mul_shr(d, 230854492u, 30u);
    for (var step = 0u; step < 4u; step++) {
        let dy2 = mul_shr(d, mul_shr(y, y, 30u), 30u);
        y = mul_shr(y, 3u * Q30 - dy2, 31u);
    }
    return y;
}

fn sphere_component(c: i32) -> i32 {
    let x = i32(min(mul_shr(u32(abs(c)), world.sphere.x, world.sphere.y), Q30));
    return tan_quarter(select(x, -x, c < 0));
}

fn sphere_scaled(v: i32, r: u32) -> i32 {
    let m = i32((mul_shr(mul_shr(u32(abs(v)), r, 29u), world.sphere.z, 30u) + 1u) >> 1u);
    return select(m, -m, v < 0);
}

// Sphere domain point of a cube point (`grid::sphere_point`): the cell's
// direction at the planet's radius, so domain distance is physical.
fn sphere_point(cube: vec3<i32>) -> vec3<i32> {
    let t = vec3<i32>(sphere_component(cube.x), sphere_component(cube.y), sphere_component(cube.z));
    let a = vec3<u32>(abs(t));
    let r = rsqrt_q30(mul_shr(a.x, a.x, 30u) + mul_shr(a.y, a.y, 30u) + mul_shr(a.z, a.z, 30u));
    return vec3<i32>(sphere_scaled(t.x, r), sphere_scaled(t.y, r), sphere_scaled(t.z, r));
}


fn volume_component(p: i32, ratio: i32) -> i32 {
    let m = i32(mul_shr(u32(abs(p)), u32(abs(ratio)), 30u));
    return select(m, -m, (p < 0) != (ratio < 0));
}

// Seamless 3D domain point of a level cell centre (`grid::volume_point`):
// the column's domain point scaled by (R + h) / R on a sphere, the height
// as the vertical axis on a plane. Volumetric terrain samples 3D noise here.
fn volume_point(face: u32, i: i32, j: i32, k: i32, level: u32) -> vec3<i32> {
    return volume_point_half(domain_point(face, i, j, level), (k << (level + 1u)) + (1 << level));
}

// Volume point at half-layer height `h` above the column with domain point `p`.
fn volume_point_half(p: vec3<i32>, h: i32) -> vec3<i32> {
    if is_plane() {
        let v = i32(mul_shr(u32(abs(h)), world.scale.w, 16u));
        return vec3<i32>(p.x, select(v, -v, h < 0), p.z);
    }
    let r = i32(mul_shr(u32(abs(h)), world.scale.y, world.scale.z));
    let ratio = select(r, -r, h < 0);
    return p + vec3<i32>(volume_component(p.x, ratio), volume_component(p.y, ratio), volume_component(p.z, ratio));
}

fn top_cells(height: i32, level: u32) -> i32 {
    return div_floor(height, world.grid.y) >> level;
}

fn terrain_kind(top: i32, k: i32) -> u32 {
    return select(0u, 1u, k < top);
}


