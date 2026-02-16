// Gizmo overlay shader - procedural rendering
// Draws transform gizmos directly in the fragment shader as an overlay

struct GizmoState {
    gizmo_type: u32,      // 0=None, 1=Translate, 2=Rotate, 3=Scale
    position: vec3<f32>,
    scale: f32,
    highlighted_axis: u32, // 0=None, 1=X, 2=Y, 3=Z
    _padding: vec3<f32>,
}

@group(2) @binding(10)
var<uniform> gizmo_state: GizmoState;

// Ray-sphere intersection for gizmo handles
fn intersect_sphere(ray_origin: vec3<f32>, ray_dir: vec3<f32>, center: vec3<f32>, radius: f32) -> f32 {
    let oc = ray_origin - center;
    let a = dot(ray_dir, ray_dir);
    let b = 2.0 * dot(oc, ray_dir);
    let c = dot(oc, oc) - radius * radius;
    let discriminant = b * b - 4.0 * a * c;
    
    if (discriminant < 0.0) {
        return -1.0;
    }
    
    return (-b - sqrt(discriminant)) / (2.0 * a);
}

// Ray-cylinder intersection for arrow shafts
fn intersect_cylinder(ray_origin: vec3<f32>, ray_dir: vec3<f32>, start: vec3<f32>, end: vec3<f32>, radius: f32) -> f32 {
    let axis = normalize(end - start);
    let oc = ray_origin - start;
    
    let a = dot(ray_dir, ray_dir) - pow(dot(ray_dir, axis), 2.0);
    let b = 2.0 * (dot(ray_dir, oc) - dot(ray_dir, axis) * dot(oc, axis));
    let c = dot(oc, oc) - pow(dot(oc, axis), 2.0) - radius * radius;
    
    let discriminant = b * b - 4.0 * a * c;
    if (discriminant < 0.0) {
        return -1.0;
    }
    
    let t = (-b - sqrt(discriminant)) / (2.0 * a);
    let hit_point = ray_origin + ray_dir * t;
    let projection = dot(hit_point - start, axis);
    let length = length(end - start);
    
    if (projection >= 0.0 && projection <= length) {
        return t;
    }
    
    return -1.0;
}

// Get axis color
fn get_axis_color(axis: u32, highlighted: bool) -> vec3<f32> {
    let intensity = select(0.8, 1.0, highlighted);
    
    if (axis == 1u) { return vec3<f32>(intensity, 0.0, 0.0); } // X = Red
    if (axis == 2u) { return vec3<f32>(0.0, intensity, 0.0); } // Y = Green
    if (axis == 3u) { return vec3<f32>(0.0, 0.0, intensity); } // Z = Blue
    
    return vec3<f32>(0.5, 0.5, 0.5);
}

// Get axis direction
fn get_axis_direction(axis: u32) -> vec3<f32> {
    if (axis == 1u) { return vec3<f32>(1.0, 0.0, 0.0); } // X
    if (axis == 2u) { return vec3<f32>(0.0, 1.0, 0.0); } // Y
    if (axis == 3u) { return vec3<f32>(0.0, 0.0, 1.0); } // Z
    return vec3<f32>(0.0, 0.0, 0.0);
}

// Render translate gizmo (arrows)
fn render_translate_gizmo(world_pos: vec3<f32>, ray_origin: vec3<f32>, ray_dir: vec3<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    var closest_t = 999999.0;
    
    // Draw 3 arrows for X, Y, Z
    for (var axis: u32 = 1u; axis <= 3u; axis = axis + 1u) {
        let dir = get_axis_direction(axis);
        let arrow_length = gizmo_state.scale * 1.5;
        let shaft_radius = gizmo_state.scale * 0.02;
        
        let start = gizmo_state.position;
        let end = gizmo_state.position + dir * arrow_length;
        
        // Check shaft intersection
        let t_shaft = intersect_cylinder(ray_origin, ray_dir, start, end, shaft_radius);
        
        // Check arrowhead (sphere at tip)
        let head_radius = gizmo_state.scale * 0.1;
        let t_head = intersect_sphere(ray_origin, ray_dir, end, head_radius);
        
        let t = min(t_shaft, t_head);
        
        if (t > 0.0 && t < closest_t) {
            closest_t = t;
            let highlighted = gizmo_state.highlighted_axis == axis;
            color = vec4<f32>(get_axis_color(axis, highlighted), 1.0);
        }
    }
    
    return color;
}

// Render rotate gizmo (circles)
fn render_rotate_gizmo(world_pos: vec3<f32>, ray_origin: vec3<f32>, ray_dir: vec3<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    var closest_t = 999999.0;
    
    let radius = gizmo_state.scale * 1.2;
    let thickness = gizmo_state.scale * 0.05;
    
    // Draw 3 circles for X, Y, Z rotations
    for (var axis: u32 = 1u; axis <= 3u; axis = axis + 1u) {
        let axis_dir = get_axis_direction(axis);
        
        // Simple torus approximation - check multiple points around circle
        for (var i: u32 = 0u; i < 32u; i = i + 1u) {
            let angle = f32(i) / 32.0 * 6.28318;
            
            var circle_point: vec3<f32>;
            if (axis == 1u) {
                circle_point = vec3<f32>(0.0, cos(angle) * radius, sin(angle) * radius);
            } else if (axis == 2u) {
                circle_point = vec3<f32>(cos(angle) * radius, 0.0, sin(angle) * radius);
            } else {
                circle_point = vec3<f32>(cos(angle) * radius, sin(angle) * radius, 0.0);
            }
            
            let world_point = gizmo_state.position + circle_point;
            let t = intersect_sphere(ray_origin, ray_dir, world_point, thickness);
            
            if (t > 0.0 && t < closest_t) {
                closest_t = t;
                let highlighted = gizmo_state.highlighted_axis == axis;
                color = vec4<f32>(get_axis_color(axis, highlighted), 1.0);
            }
        }
    }
    
    return color;
}

// Render scale gizmo (cubes at ends)
fn render_scale_gizmo(world_pos: vec3<f32>, ray_origin: vec3<f32>, ray_dir: vec3<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    var closest_t = 999999.0;
    
    // Draw 3 cubes for X, Y, Z scale handles
    for (var axis: u32 = 1u; axis <= 3u; axis = axis + 1u) {
        let dir = get_axis_direction(axis);
        let handle_pos = gizmo_state.position + dir * gizmo_state.scale;
        let handle_size = gizmo_state.scale * 0.15;
        
        let t = intersect_sphere(ray_origin, ray_dir, handle_pos, handle_size);
        
        if (t > 0.0 && t < closest_t) {
            closest_t = t;
            let highlighted = gizmo_state.highlighted_axis == axis;
            color = vec4<f32>(get_axis_color(axis, highlighted), 1.0);
        }
    }
    
    return color;
}

// Main gizmo rendering function
fn render_gizmo_overlay(world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec4<f32> {
    // Skip if no gizmo active
    if (gizmo_state.gizmo_type == 0u) {
        return vec4<f32>(0.0);
    }
    
    // Create ray from camera through pixel
    let ray_dir = normalize(world_pos - camera_pos);
    
    // Render appropriate gizmo type
    if (gizmo_state.gizmo_type == 1u) {
        return render_translate_gizmo(world_pos, camera_pos, ray_dir);
    } else if (gizmo_state.gizmo_type == 2u) {
        return render_rotate_gizmo(world_pos, camera_pos, ray_dir);
    } else if (gizmo_state.gizmo_type == 3u) {
        return render_scale_gizmo(world_pos, camera_pos, ray_dir);
    }
    
    return vec4<f32>(0.0);
}
