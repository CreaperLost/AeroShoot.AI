struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) local: vec2<f32>,
}

struct LayerParams {
    size_px: vec2<f32>,
    radius_px: f32,
    clip_mode: u32,
    shadow_blur_px: f32,
    shadow_opacity: f32,
    shadow_offset: vec2<f32>,
    pass_kind: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var layer_tex: texture_2d<f32>;
@group(0) @binding(1) var layer_samp: sampler;
@group(0) @binding(2) var<uniform> params: LayerParams;

@vertex
fn vs_main(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) local: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    out.local = local;
    return out;
}

fn sdf_rounded_rect(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let r = max(min(radius, min(half.x, half.y)), 0.0);
    let q = abs(p) - (half - vec2<f32>(r, r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sdf_circle(p: vec2<f32>, half: vec2<f32>) -> f32 {
    return length(p) - min(half.x, half.y);
}

fn sdf_squircle(p: vec2<f32>, half: vec2<f32>) -> f32 {
    let nx = select(0.0, abs(p.x) / half.x, half.x > 0.0);
    let ny = select(0.0, abs(p.y) / half.y, half.y > 0.0);
    let k = (nx * nx * nx * nx) + (ny * ny * ny * ny);
    let r = min(half.x, half.y);
    return pow(max(k, 0.0), 0.25) * r - r;
}

fn shape_sdf(p: vec2<f32>) -> f32 {
    let half = params.size_px * 0.5;
    switch params.clip_mode {
        case 2u: {
            return sdf_circle(p, half);
        }
        case 3u: {
            return sdf_squircle(p, half);
        }
        default: {
            return sdf_rounded_rect(p, half, params.radius_px);
        }
    }
}

fn shadow_coverage(sdf: f32, blur: f32) -> f32 {
    if blur <= 0.0 {
        return select(0.0, 1.0, sdf <= 0.0);
    }
    return 1.0 - clamp(sdf / blur, 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if params.pass_kind == 1u {
        let p = (in.local - vec2<f32>(0.5)) * params.size_px - params.shadow_offset;
        let sdf = shape_sdf(p);
        let alpha = params.shadow_opacity * shadow_coverage(sdf, params.shadow_blur_px);
        return vec4<f32>(0.0, 0.0, 0.0, alpha);
    }
    let p = (in.local - vec2<f32>(0.5)) * params.size_px;
    let sdf = shape_sdf(p);
    if sdf > 0.0 {
        return vec4<f32>(0.0);
    }
    return textureSample(layer_tex, layer_samp, in.uv);
}
