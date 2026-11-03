// Pass 2: glyph quads from the atlas. One instance per glyph; the quad's
// four corners come from the vertex index (a triangle strip).

struct Uniforms {
    // Maps pixel coordinates to wgpu NDC (see shape.rs).
    ortho: mat4x4<f32>,
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
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
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
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Quads sit on whole pixels, so each fragment maps to exactly one texel
    // and no filtering is needed.
    let texel = vec2<i32>(floor(in.uv));
    if in.kind == 1u {
        return textureLoad(color_atlas, texel, 0);
    }
    let coverage = textureLoad(coverage_atlas, texel, 0).r * in.color.a;
    // Premultiplied output.
    return vec4<f32>(in.color.rgb * coverage, coverage);
}
