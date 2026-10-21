// The GPU map's live raster tile layer (tilelayer.rs): each tile an instance, its picture a layer
// of a texture array, placed for the camera of the frame drawn.

struct LiveUniforms {
    /// Map units (from the camera's centre) to clip space: 2 / (view size × resolution).
    scale: vec2f,
    _pad: vec2f,
};

struct LiveIn {
    /// The square drawn: least x and y, greatest x and y, map units from the camera's centre.
    @location(0) rect: vec4f,
    /// The part of the picture in it: left, top, right, bottom (0..1, top down).
    @location(1) uv: vec4f,
    /// The picture's layer in the texture array.
    @location(2) layer: u32,
    @location(3) opacity: f32,
};

struct LiveOut {
    @builtin(position) clip: vec4f,
    @location(0) uv: vec2f,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) opacity: f32,
};

@group(0) @binding(0) var<uniform> u_live: LiveUniforms;
@group(0) @binding(1) var t_live: texture_2d_array<f32>;
@group(0) @binding(2) var s_live: sampler;

@vertex
fn vs_live(in: LiveIn, @builtin(vertex_index) i: u32) -> LiveOut {
    // Two triangles: bottom left, bottom right, top right; bottom left, top right, top left.
    var corners = array<vec2f, 6>(vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 0.0), vec2f(1.0, 1.0), vec2f(0.0, 1.0));
    let c = corners[i % 6u];
    let p = mix(in.rect.xy, in.rect.zw, c);
    var out: LiveOut;
    out.clip = vec4f(p * u_live.scale, 0.0, 1.0);
    // The picture's top row is the square's top (greatest y).
    out.uv = vec2f(mix(in.uv.x, in.uv.z, c.x), mix(in.uv.w, in.uv.y, c.y));
    out.layer = in.layer;
    out.opacity = in.opacity;
    return out;
}

@fragment
fn fs_live(in: LiveOut) -> @location(0) vec4f {
    let size = vec2f(textureDimensions(t_live).xy);
    let blended = textureSample(t_live, s_live, in.uv, in.layer);
    // A tile drawn at its own size takes its nearest pixel, as the CPU path does, rather than a
    // blend of four that would soften it; scaled, it is blended.
    let texels = fwidth(in.uv * size);
    var c = blended;
    if all(abs(texels - vec2f(1.0)) < vec2f(0.01)) {
        c = textureLoad(t_live, vec2i(clamp(floor(in.uv * size), vec2f(0.0), size - 1.0)), in.layer, 0);
    }
    return c * in.opacity;
}
