// The material row in WGSL (helio-mats `MATERIAL_ROW_SNIPPET`,
// `//!use material_row`): one `GpuMaterial` per material in the `materials`
// table (`helio_mats::MATERIALS`), indexed by the material id a kind's own
// rows carry, with its flags and classes, and `MaterialInput`, the point of
// a surface a drawer hands a material's template. Every kind that reads a
// material reads it through this.

// `helio_mats::GpuMaterial` / `MaterialRow` (96 bytes).
struct GpuMaterial {
    // Linear RGBA.
    base_color:         vec4<f32>,
    // Linear RGB and a strength (w).
    emissive:           vec4<f32>,
    // x roughness, y metallic (specular strength in the specular workflow),
    // z index of refraction, w specular tint.
    roughness_metallic: vec4<f32>,
    // Material texture table slots (`MATERIAL_NO_TEXTURE`: none).
    tex_base_color:     u32,
    tex_normal:         u32,
    tex_roughness:      u32,
    tex_emissive:       u32,
    tex_occlusion:      u32,
    // `MATERIAL_WORKFLOW_*`.
    workflow:           u32,
    // `MATERIAL_FLAG_*`.
    flags:              u32,
    // `MATERIAL_CLASS_*`: the Radiant template the surface is shaded with.
    material_class:     u32,
    // What the class's template reads (subsurface colour and radius, ...).
    class_params:       vec4<f32>,
}

// The point of a surface a material shades, as the drawer sees it.
struct MaterialInput {
    clip_position:  vec4<f32>,
    world_position: vec3<f32>,
    // The geometric normal (unit), the tangent along +u and the
    // bitangent's sign: the normal map's frame.
    world_normal:   vec3<f32>,
    world_tangent:  vec3<f32>,
    bitangent_sign: f32,
    tex_coords:     vec2<f32>,
    // Where the view is (world space, as `world_position`): what a
    // view-dependent material (glass, water, opal) looks from.
    view_position:  vec3<f32>,
    // The frame's number, for a material animated by frames.
    frame:          u32,
}

// The row as words, for a reader that binds the table as `array<u32>`.
const MATERIAL_WORDS: u32 = 24u;
const MATERIAL_FLAGS_WORD: u32 = 18u;
const MATERIAL_CLASS_WORD: u32 = 19u;

// No texture in a `tex_*` slot (`helio_mats::GpuMaterial::NO_TEXTURE`).
const MATERIAL_NO_TEXTURE: u32 = 0xffffffffu;

// `helio_mats::MaterialWorkflow`.
const MATERIAL_WORKFLOW_METALLIC: u32 = 0u;
const MATERIAL_WORKFLOW_SPECULAR: u32 = 1u;

// `helio_mats::FLAG_*`.
const MATERIAL_FLAG_DOUBLE_SIDED: u32 = 1u;
const MATERIAL_FLAG_ALPHA_BLEND: u32 = 2u;
const MATERIAL_FLAG_ALPHA_TEST: u32 = 4u;
const MATERIAL_FLAG_HAS_NORMAL_MAP: u32 = 8u;
const MATERIAL_FLAG_HAS_CLEAR_COAT: u32 = 16u;
const MATERIAL_FLAG_HAS_SUBSURFACE: u32 = 32u;
const MATERIAL_FLAG_HAS_ANISOTROPY: u32 = 64u;
const MATERIAL_FLAG_HAS_CUSTOM_SHADER: u32 = 128u;
const MATERIAL_FLAG_TRANSPARENT_ONLY: u32 = 256u;
const MATERIAL_FLAG_FORWARD_SHADING: u32 = 512u;

// `helio_mats::MATERIAL_CLASS_*`.
const MATERIAL_CLASS_DEFAULT: u32 = 0u;
const MATERIAL_CLASS_CLEAR_COAT: u32 = 1u;
const MATERIAL_CLASS_SUBSURFACE: u32 = 2u;
const MATERIAL_CLASS_ANISOTROPIC: u32 = 3u;
const MATERIAL_CLASS_SKIN: u32 = 4u;
const MATERIAL_CLASS_FOLIAGE: u32 = 5u;
const MATERIAL_CLASS_CUSTOM: u32 = 0xffffu;

// The alpha a cut-out material's texels are cut below, when its texture
// metadata gives none.
const MATERIAL_DEFAULT_ALPHA_CUTOFF: f32 = 0.5;

// The material standing in where no material table is bound (the materials
// plugin absent): default grey PBR, rough and dielectric, no textures.
fn material_stand_in() -> GpuMaterial {
    return GpuMaterial(
        vec4<f32>(0.5, 0.5, 0.5, 1.0),
        vec4<f32>(0.0),
        vec4<f32>(0.5, 0.0, 1.5, 0.5),
        MATERIAL_NO_TEXTURE, MATERIAL_NO_TEXTURE, MATERIAL_NO_TEXTURE,
        MATERIAL_NO_TEXTURE, MATERIAL_NO_TEXTURE,
        MATERIAL_WORKFLOW_METALLIC, 0u, MATERIAL_CLASS_DEFAULT, vec4<f32>(0.0),
    );
}

// The share of the light on one side of `material` that leaves the other
// (two-sided foliage, `MATERIAL_CLASS_FOLIAGE`'s `class_params.x`; 0:
// opaque): what a tracer of the light through a surface reads.
fn material_transmission(material: GpuMaterial) -> f32 {
    return select(0.0, clamp(material.class_params.x, 0.0, 1.0), material.material_class == MATERIAL_CLASS_FOLIAGE);
}

// The light a thin sheet of `material` lets through (tinted glass, by
// Beer-Lambert): its base colour is the transmittance of a reference sheet,
// its alpha the sheet's optical thickness (alpha 0.5 transmits the colour,
// 0 is clear). Matches the depth of colour of ray-traced thin-sheet
// transmission far better than the display blend mix(1, c, a), which reads
// as pastel.
fn material_sheet_transmittance(material: GpuMaterial) -> vec3<f32> {
    let c = clamp(material.base_color.rgb, vec3<f32>(0.02), vec3<f32>(1.0));
    let a = clamp(material.base_color.a, 0.0, 1.0);
    return pow(c, vec3<f32>(2.0 * a));
}
