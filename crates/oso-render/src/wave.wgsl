// Wavy lines, one instanced quad each (ADR 0138): a sine that swings between the top and the
// bottom of its band, antialiased by its distance to the curve. Its phase counts from the
// frame's left, so lines side by side join. Output is premultiplied.

struct Uniforms {
    // Maps pixel coordinates to wgpu NDC (see shape.rs).
    ortho: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct Instance {
    // The band: x, y, width, height, in pixels.
    @location(0) rect: vec4<f32>,
    // Linear RGBA, straight alpha.
    @location(1) color: vec4<f32>,
    // The line's thickness and its period, in pixels.
    @location(2) params: vec4<f32>,
    // The quad, cut to its pane on the CPU: x0, y0, x1, y1.
    @location(3) bounds: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) point: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOutput {
    // 0: (0, 0), 1: (1, 0), 2: (0, 1), 3: (1, 1)
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let point = mix(instance.bounds.xy, instance.bounds.zw, corner);
    var out: VertexOutput;
    out.clip_position = uniforms.ortho * vec4<f32>(point, 0.0, 1.0);
    out.point = point;
    out.rect = instance.rect;
    out.color = instance.color;
    out.params = instance.params;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let thickness = in.params.x;
    let k = 6.2831853 / max(in.params.y, 1.0);
    let amplitude = max(in.rect.w - thickness, 0.0) * 0.5;
    let middle = in.rect.y + in.rect.w * 0.5;
    let phase = in.point.x * k;
    // The vertical distance to the curve, over the slope's length: near the distance to it.
    let slope = amplitude * k * cos(phase);
    let d = abs(in.point.y - (middle - amplitude * sin(phase))) / sqrt(1.0 + slope * slope);
    let coverage = clamp(thickness * 0.5 + 0.5 - d, 0.0, 1.0);
    let alpha = in.color.a * coverage;
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
