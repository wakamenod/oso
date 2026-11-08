// Rounded boxes with an outline and a soft shadow, one instanced quad each (ADR 0075).
// The corners and the outline come from the box's signed distance, and the shadow from a
// closed-form approximation of a Gaussian blur of a rounded box (Evan Wallace's), so there
// is no blur pass. Output is premultiplied.

struct Uniforms {
    // Maps pixel coordinates to wgpu NDC (see shape.rs).
    ortho: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct Instance {
    // The box: x, y, width, height, in pixels.
    @location(0) rect: vec4<f32>,
    // Corner radii: top left, top right, bottom right, bottom left.
    @location(1) radii: vec4<f32>,
    // Linear RGBA, straight alpha.
    @location(2) fill: vec4<f32>,
    // The outline's color, drawn outside the box.
    @location(3) outline: vec4<f32>,
    @location(4) shadow: vec4<f32>,
    // The outline's width, the shadow's offset down, its sigma, and the quad's margin.
    @location(5) params: vec4<f32>,
    // Nothing is drawn outside this: x0, y0, x1, y1.
    @location(6) clip: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) point: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) radii: vec4<f32>,
    @location(3) @interpolate(flat) fill: vec4<f32>,
    @location(4) @interpolate(flat) outline: vec4<f32>,
    @location(5) @interpolate(flat) shadow: vec4<f32>,
    @location(6) @interpolate(flat) params: vec4<f32>,
    @location(7) @interpolate(flat) clip: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32, instance: Instance) -> VertexOutput {
    // 0: (0, 0), 1: (1, 0), 2: (0, 1), 3: (1, 1)
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    let margin = instance.params.w;
    let low = max(instance.rect.xy - vec2<f32>(margin), instance.clip.xy);
    let high = min(instance.rect.xy + instance.rect.zw + vec2<f32>(margin), instance.clip.zw);
    let point = mix(low, max(low, high), corner);
    var out: VertexOutput;
    out.clip_position = uniforms.ortho * vec4<f32>(point, 0.0, 1.0);
    out.point = point;
    out.rect = instance.rect;
    out.radii = instance.radii;
    out.fill = instance.fill;
    out.outline = instance.outline;
    out.shadow = instance.shadow;
    out.params = instance.params;
    out.clip = instance.clip;
    return out;
}

// The signed distance from `p`, relative to the box's center, to a box of `half` its size
// with the corner radii `r`. Y goes down.
fn rounded_box(p: vec2<f32>, half: vec2<f32>, r: vec4<f32>) -> f32 {
    let top = select(r.x, r.y, p.x > 0.0);
    let bottom = select(r.w, r.z, p.x > 0.0);
    let radius = select(bottom, top, p.y < 0.0);
    let q = abs(p) - half + vec2<f32>(radius);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - radius;
}

fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (2.5066283 * sigma);
}

// An approximation of erf, good to about 5e-4.
fn erf2(x: vec2<f32>) -> vec2<f32> {
    let s = sign(x);
    let a = abs(x);
    var r = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
    r = r * r;
    return s - s / (r * r);
}

// The blurred box's coverage along x at height `y`, for a box of `half` its size with
// corners of `corner`.
fn shadow_x(x: f32, y: f32, sigma: f32, corner: f32, half: vec2<f32>) -> f32 {
    let delta = min(half.y - corner - abs(y), 0.0);
    let curved = half.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf2((vec2<f32>(x) + vec2<f32>(-curved, curved)) * (0.70710678 / sigma));
    return integral.y - integral.x;
}

// A rounded box blurred by `sigma`, sampled at `p` relative to its center, from four steps
// of the integral along y.
fn shadow_at(p: vec2<f32>, half: vec2<f32>, sigma: f32, corner: f32) -> f32 {
    let low = p.y - half.y;
    let high = p.y + half.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var value = 0.0;
    for (var i = 0; i < 4; i++) {
        value += shadow_x(p.x, p.y - y, sigma, corner, half) * gaussian(y, sigma) * step;
        y += step;
    }
    return value;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if any(in.point < in.clip.xy) || any(in.point >= in.clip.zw) {
        discard;
    }
    let half = in.rect.zw * 0.5;
    let center = in.rect.xy + half;
    let d = rounded_box(in.point - center, half, in.radii);
    let width = in.params.x;
    // Coverage with a pixel of antialiasing.
    let inside = clamp(0.5 - d, 0.0, 1.0);
    let ring = clamp(0.5 - (d - width), 0.0, 1.0) - inside;
    let fill = in.fill.a * inside;
    let line = in.outline.a * select(0.0, ring, width > 0.0);
    let front = vec4<f32>(in.fill.rgb * fill + in.outline.rgb * line, fill + line);
    var back = 0.0;
    let sigma = in.params.z;
    if in.shadow.a > 0.0 && sigma > 0.0 {
        let offset = vec2<f32>(0.0, in.params.y);
        let corner = max(max(in.radii.x, in.radii.y), max(in.radii.z, in.radii.w));
        back = in.shadow.a * shadow_at(in.point - center - offset, half, sigma, corner);
    }
    let under = vec4<f32>(in.shadow.rgb * back, back) * (1.0 - front.a);
    return front + under;
}
