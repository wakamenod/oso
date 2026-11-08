// Glyph quads from the atlas. One instance per glyph; the quad's
// four corners come from the vertex index (a triangle strip).

struct Uniforms {
    // Maps pixel coordinates to wgpu NDC (see shape.rs).
    ortho: mat4x4<f32>,
    // x: the strength of the theme's text gamma, 0 to 1 (ADR 0089).
    gamma: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
// One byte of coverage per pixel.
@group(0) @binding(1) var coverage_atlas: texture_2d<f32>;
// Premultiplied RGBA (color emoji).
@group(0) @binding(2) var color_atlas: texture_2d<f32>;

struct Instance {
    // Top-left corner on screen, in whole pixels.
    @location(0) position: vec2<f32>,
    @location(1) size: vec2<f32>,
    // Top-left corner in the atlas, in pixels.
    @location(2) uv: vec2<f32>,
    // Linear RGBA, used for coverage glyphs only.
    @location(3) color: vec4<f32>,
    // 0: coverage, 1: color.
    @location(4) kind: u32,
    // The luminance of what the glyph is drawn on.
    @location(5) paper: f32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
    @location(3) @interpolate(flat) paper: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOutput {
    // 0: (0, 0), 1: (1, 0), 2: (0, 1), 3: (1, 1)
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let offset = corner * instance.size;
    var out: VertexOutput;
    out.clip_position = uniforms.ortho * vec4<f32>(instance.position + offset, 0.0, 1.0);
    out.uv = instance.uv + offset;
    out.color = instance.color;
    out.kind = instance.kind;
    out.paper = instance.paper;
    return out;
}

// An sRGB value in linear light, and back.
fn decode(v: f32) -> f32 {
    return select(pow((v + 0.055) / 1.055, 2.4), v / 12.92, v <= 0.04045);
}

fn encode(v: f32) -> f32 {
    return select(1.055 * pow(v, 1.0 / 2.4) - 0.055, v * 12.92, v <= 0.0031308);
}

// The coverage that, blended in linear light, gives what `coverage` blended in sRGB gives,
// for text of luminance `ink` on `paper`, at `strength`. As theme.rs's corrected_coverage.
fn corrected(coverage: f32, ink: f32, paper: f32, strength: f32) -> f32 {
    if abs(ink - paper) < 1e-3 {
        return coverage;
    }
    let native = decode(coverage * encode(ink) + (1.0 - coverage) * encode(paper));
    let wanted = clamp((native - paper) / (ink - paper), 0.0, 1.0);
    return mix(coverage, wanted, strength);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Quads sit on whole pixels, so each fragment maps to exactly one texel
    // and no filtering is needed.
    let texel = vec2<i32>(floor(in.uv));
    if in.kind == 1u {
        return textureLoad(color_atlas, texel, 0);
    }
    let ink = dot(in.color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let sample = textureLoad(coverage_atlas, texel, 0).r;
    let coverage = corrected(sample, ink, in.paper, uniforms.gamma.x) * in.color.a;
    // Premultiplied output.
    return vec4<f32>(in.color.rgb * coverage, coverage);
}
