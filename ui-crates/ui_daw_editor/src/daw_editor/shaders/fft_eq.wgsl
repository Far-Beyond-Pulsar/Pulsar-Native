@group(0) @binding(0) var<storage, read> eq_curve: array<f32>;
@group(0) @binding(1) var<storage, read> input: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;

// Each invocation computes one output sample
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    // Set up indices and lengths
    let idx = global_id.x;
    let input_len = arrayLength(&input);
    
    // Guard against out-of-bounds access
    if (idx >= input_len) {
        return;
    }
    
    // Apply EQ curve
    let gain = eq_curve[idx % arrayLength(&eq_curve)];

    // Write the result to the output buffer
    output[idx] = input[idx] * gain;
}
