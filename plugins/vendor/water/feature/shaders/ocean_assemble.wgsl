// Convert packed inverse FFT fields into the exact Ultra-Water output channels.
@group(0) @binding(0) var input0: texture_2d<f32>;
@group(0) @binding(1) var input1: texture_2d<f32>;
@group(0) @binding(2) var previous_foam: texture_2d_array<f32>;

struct AssembleUniform {
    lambda: f32,
    dt: f32,
    foam_recovery: f32,
    previous_valid: f32,
}
@group(0) @binding(3) var<uniform> u: AssembleUniform;

const N: i32 = 256;

struct FragmentOutput {
    @location(0) displacement: vec4f,
    @location(1) slope_moments: vec4f,
    @location(2) foam_history: vec4f,
}

@fragment
fn fs_main(
    @builtin(position) position: vec4f,
    @location(1) @interpolate(flat) cascade: u32,
) -> FragmentOutput {
    let id = vec2i(position.xy);
    let atlas = vec2i(id.x, id.y + i32(cascade) * N);
    let checkerboard = select(-1.0, 1.0, ((id.x + id.y) & 1) == 0);
    let a = textureLoad(input0, atlas, 0) * checkerboard; // Dx, Dz, Dy, Dxz
    let b = textureLoad(input1, atlas, 0) * checkerboard; // Dyx, Dyz, Dxx, Dzz
    let dxx = u.lambda * b.z;
    let dzz = u.lambda * b.w;
    let dxz = u.lambda * a.w;
    let jacobian = (1.0 + dxx) * (1.0 + dzz) - dxz * dxz;
    let slope = b.xy / max(vec2f(1.0 + dxx, 1.0 + dzz), vec2f(0.2));
    var previous = 1.0;
    if u.previous_valid > 0.5 {
        previous = textureLoad(previous_foam, id, i32(cascade), 0).r;
    }
    // Foam descends immediately with the Jacobian and recovers at a dt-based rate.
    let turbulence = min(jacobian, previous + u.dt * u.foam_recovery / max(jacobian, 0.5));
    var out: FragmentOutput;
    out.displacement = vec4f(u.lambda * a.x, a.z, u.lambda * a.y, turbulence);
    out.slope_moments = vec4f(slope, slope * slope);
    out.foam_history = vec4f(turbulence, 0.0, 0.0, 0.0);
    return out;
}
