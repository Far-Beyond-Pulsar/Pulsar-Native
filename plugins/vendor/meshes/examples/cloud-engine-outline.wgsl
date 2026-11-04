struct OutlineUniform {
    camera: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    params: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> u: OutlineUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(input: VsIn) -> VsOut {
    let local = input.position - u.camera.xyz;
    let x = dot(local, u.right.xyz);
    let y = dot(local, u.up.xyz);
    let z = dot(local, u.forward.xyz);
    let clip_w = max(z, u.params.z);

    return VsOut(
        vec4<f32>(
            x * u.params.x * u.params.y,
            y * u.params.x,
            0.0,
            clip_w,
        ),
    );
}

@fragment
fn fs_main(v: VsOut) -> @location(0) vec4<f32> {
    _ = v;
    return u.color;
}
