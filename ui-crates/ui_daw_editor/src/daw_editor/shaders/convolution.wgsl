@group(0) @binding(0) var<storage, read> impulse_response: array<f32>;
@group(0) @binding(1) var<storage, read> input: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;

// Each invocation computes one output sample
@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {

    // Set up indices and lengths
    let out_idx = global_id.x;
    let ir_len = arrayLength(&impulse_response);
    let input_len = arrayLength(&input);
    let output_len = arrayLength(&output);
    
    // Guard against out-of-bounds access
    if (out_idx >= output_len) {
        return;
    }
    
    // Perform convolution
    var sum: f32 = 0.0;
    for (var i: u32 = 0u; i < ir_len; i = i + 1u) {
        if (out_idx >= i && (out_idx - i) < input_len) {
            sum = sum + impulse_response[i] * input[out_idx - i];
        }
    }
    
    // Write the result to the output buffer
    output[out_idx] = sum;
}
