// Element-wise kernels (one thread per pixel, 1-D on a 2-D grid). Bindings: a, b, c (inputs), dst.
// P[0] = pixel count; further parameters per kernel. Each mirrors a CPU expression in
// `lightcraft_pipeline::local` (named in the comment).

fn rgb_a(i: u32) -> vec3<f32> {
    return vec3<f32>(a[3u * i], a[3u * i + 1u], a[3u * i + 2u]);
}

fn put_rgb(i: u32, v: vec3<f32>) {
    dst[3u * i] = v.x;
    dst[3u * i + 1u] = v.y;
    dst[3u * i + 2u] = v.z;
}

// `log_lum` of an RGB image.
@compute @workgroup_size(256)
fn log_lum_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = log_lum(rgb_a(i));
}

// `dark_of`: the dark channel (dehaze).
@compute @workgroup_size(256)
fn dark_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let c = rgb_a(i);
    dst[i] = min(min(c.x, c.y), c.z);
}

// Guided filter input pair (p, p²).
@compute @workgroup_size(256)
fn guided_pre(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let p = a[i];
    dst[2u * i] = p;
    dst[2u * i + 1u] = p * p;
}

// Guided filter coefficients (a, b) from blurred (mean, corr). P[1] = eps.
@compute @workgroup_size(256)
fn guided_ab(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let m = a[2u * i];
    let c = a[2u * i + 1u];
    let v = max(c - m * m, 0.0);
    let k = v / (v + pf(1u));
    dst[2u * i] = k;
    dst[2u * i + 1u] = m - k * m;
}

// `q = a·p + b` (a: p, b: interleaved blurred coefficients).
@compute @workgroup_size(256)
fn guided_apply(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = b[2u * i] * a[i] + b[2u * i + 1u];
}

// Cross guided filter (`masks::guided_cross`), a: guide, b: input. P[1] = 0: (guide, input);
// 1: (guide·input, guide²).
@compute @workgroup_size(256)
fn xguided_pre(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let gi = a[i];
    let p = b[i];
    if (pu(1u) == 0u) {
        dst[2u * i] = gi;
        dst[2u * i + 1u] = p;
    } else {
        dst[2u * i] = gi * p;
        dst[2u * i + 1u] = gi * gi;
    }
}

// Coefficients (a, b) from blurred means (a: guide, input) and correlations (b: guide·input,
// guide²). P[1] = eps.
@compute @workgroup_size(256)
fn xguided_ab(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let mi = a[2u * i];
    let mp = a[2u * i + 1u];
    let v = max(b[2u * i + 1u] - mi * mi, 0.0);
    let k = (b[2u * i] - mi * mp) / (v + pf(1u));
    dst[2u * i] = k;
    dst[2u * i + 1u] = mp - k * mi;
}

// `max(p, clamp(a·guide + b, 0, 1))` (a: guide, b: blurred coefficients, c: the input p).
@compute @workgroup_size(256)
fn xguided_apply(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = max(c[i], clamp(b[2u * i] * a[i] + b[2u * i + 1u], 0.0, 1.0));
}

// White balance: 3×3 matrix (P[1] = has matrix, P[2..11] row-major), clamped at 0.
@compute @workgroup_size(256)
fn wb_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    var c = rgb_a(i);
    if (pu(1u) != 0u) {
        c = vec3<f32>(
            pf(2u) * c.x + pf(3u) * c.y + pf(4u) * c.z,
            pf(5u) * c.x + pf(6u) * c.y + pf(7u) * c.z,
            pf(8u) * c.x + pf(9u) * c.y + pf(10u) * c.z,
        );
    }
    put_rgb(i, max(c * 1.0, vec3<f32>(0.0)));
}

// Luminance NR: scale by 2^((f − l)·k) (a: image, b: log luminance l, c: filtered f). P[1] = k.
@compute @workgroup_size(256)
fn nr_lum(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let d = (c[i] - b[i]) * pf(1u);
    put_rgb(i, rgb_a(i) * exp2(d));
}

// Chromaticity rgb / Y.
@compute @workgroup_size(256)
fn chroma_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let c0 = rgb_a(i);
    let y = max(lum2020(c0), 1e-6);
    put_rgb(i, vec3<f32>(c0.x / y, c0.y / y, c0.z / y));
}

// Colour NR: blend chromaticity towards its blur and re-apply luminance (a: image, b: chroma,
// c: blurred chroma). P[1] = t.
@compute @workgroup_size(256)
fn nr_col(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let yl = lum2020(rgb_a(i));
    let t = pf(1u);
    let c0 = vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]);
    let cb = vec3<f32>(c[3u * i], c[3u * i + 1u], c[3u * i + 2u]);
    put_rgb(i, max((c0 + (cb - c0) * t) * yl, vec3<f32>(0.0)));
}

// The sharpening weight of each pixel (`finish::sharpen_mask_weight`) for the Masking preview
// (`Overlay::SharpenMask`), a: log luminance, b: the sharpening band's blur of it. P[1] = Masking (0..1).
@compute @workgroup_size(256)
fn sharpen_mask_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let k = pf(1u);
    if (k > 0.0) {
        dst[i] = smoothstep(k * 0.25, k * 0.25 + 0.15, abs(a[i] - b[i]));
    } else {
        dst[i] = 1.0;
    }
}

// The guide of V3 colour NR (`local::nr_guide`): the cube root of luminance.
fn nr_guide(c: vec3<f32>) -> f32 {
    let y = lum2020(c);
    return select(0.0, pow(y, 1.0 / 3.0), y > 0.0);
}

// V3 colour NR inputs (`local::nr_color_guided`), a: image, b: chromaticity. P[1] = 0: (g, g²);
// 1: chromaticity · g (3 channels).
@compute @workgroup_size(256)
fn nrx_pre(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let gv = nr_guide(rgb_a(i));
    if (pu(1u) == 0u) {
        dst[2u * i] = gv;
        dst[2u * i + 1u] = gv * gv;
    } else {
        put_rgb(i, vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]) * gv);
    }
}

// V3 colour NR coefficients from blurred (a: g, g²), (b: chromaticity), (c: chromaticity · g).
// P[1] = eps; P[2] = 0: the slopes a, 1: the offsets b (3 channels each).
@compute @workgroup_size(256)
fn nrx_ab(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let mg = a[2u * i];
    let v = max(a[2u * i + 1u] - mg * mg, 0.0);
    let mp = vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]);
    let mgp = vec3<f32>(c[3u * i], c[3u * i + 1u], c[3u * i + 2u]);
    let k = (mgp - mg * mp) / (v + pf(1u));
    if (pu(2u) == 0u) {
        put_rgb(i, k);
    } else {
        put_rgb(i, mp - k * mg);
    }
}

// V3 colour NR's filtered chromaticity (`local::nr_color_filtered`): max(ma·g + mb, 0) (a: image,
// b: blurred slopes, c: blurred offsets).
@compute @workgroup_size(256)
fn nrx_q(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let q = vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]) * nr_guide(rgb_a(i)) + vec3<f32>(c[3u * i], c[3u * i + 1u], c[3u * i + 2u]);
    put_rgb(i, max(q, vec3<f32>(0.0)));
}

// What V3 colour NR takes away, squared (`local::removed_energy` before its blur): |chroma − q|²
// (a: chromaticity, b: filtered chromaticity).
@compute @workgroup_size(256)
fn nrx_energy(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let d = rgb_a(i) - vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]);
    dst[i] = dot(d, d);
}

// The colour noise at guide value `gv` (`ChromaNoise::at`), from the 16 bands at P[3..19].
fn nrx_noise(gv: f32) -> f32 {
    var x = clamp(gv / 1.25 * 16.0 - 0.5, 0.0, 15.0);
    if (x != x) {
        x = 0.0;
    }
    let i = min(u32(x), 14u);
    let f = x - f32(i);
    let n0 = pf(3u + i);
    return n0 + (pf(4u + i) - n0) * f;
}

// V3 colour NR (`local::nr_color_guided`): chromaticity towards the filtered `q`, with what it
// takes away kept by `local::keep_weight`, luminance re-applied (a: image, b: q, c: local energy of
// what is taken away). P[1] = t, P[2] = keep, P[3..19] = the colour noise's bands.
@compute @workgroup_size(256)
fn nrx_apply(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let p = rgb_a(i);
    let yl = lum2020(p);
    let c0 = p / max(yl, 1e-6);
    let q = vec3<f32>(b[3u * i], b[3u * i + 1u], b[3u * i + 2u]);
    let e = c[i];
    let bar = pf(2u) * nrx_noise(nr_guide(p));
    var k = 0.0;
    if (e > 0.0 && e <= 3.4e38) {
        k = clamp(1.0 - bar / e, 0.0, 1.0);
    }
    let n = c0 + (q + (c0 - q) * k - c0) * pf(1u);
    let ln = lum2020(n);
    if (ln > 1e-6) {
        put_rgb(i, max(n * (yl / ln), vec3<f32>(0.0)));
    } else {
        put_rgb(i, max(c0 * yl, vec3<f32>(0.0)));
    }
}

// The finish kernel's `tex` binding under V3 sharpening (`params::Present::tex_stride`), P[1] =
// values a pixel: a (the Texture band, or any plane when there is none), b (the sharpening band),
// c (its edge map, when P[2] != 0).
@compute @workgroup_size(256)
fn pack_tex(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let s = pu(1u);
    dst[s * i] = a[i];
    dst[s * i + 1u] = b[i];
    if (pu(2u) != 0u) {
        dst[s * i + 2u] = c[i];
    }
}

// dst += P[1]·a.
@compute @workgroup_size(256)
fn axpy_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = dst[i] + pf(1u) * a[i];
}

// The Radius / Detail preview (`visualize::sharpen_detail`): grey 0.5 + P[1]·v, v = a (V3's
// The Radius / Detail preview before process V3 (`visualize::sharpen_detail`): grey 0.5 + P[1]·(a − b),
// a: log luminance, b: the Texture band's blur of it.
@compute @workgroup_size(256)
fn detail_view_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = clamp(0.5 + pf(1u) * (a[i] - b[i]), 0.0, 1.0);
}

// Process V3's sharpening: the square root of luminance relative to middle grey's (`local::sqrt_lum_plane`).
@compute @workgroup_size(256)
fn sqrt_lum_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = exp2(0.5 * a[i]);
}

// `tone::ToneMap::apply` of the tone table in `c` (TONE_N entries).
fn tone_c(y: f32) -> f32 {
    if (y <= 0.0) {
        return 0.0;
    }
    let ev = log2(y / GREY);
    let f = clamp((ev - TONE_MIN_EV) / (TONE_MAX_EV - TONE_MIN_EV), 0.0, 1.0) * f32(TONE_N - 1u);
    let k = min(u32(f), TONE_N - 2u);
    let t = f - f32(k);
    let v = c[k] + (c[k + 1u] - c[k]) * t;
    if (ev < TONE_MIN_EV) {
        return v * (y / (GREY * TONE_MIN_GAIN));
    }
    return v;
}

// The Masking preview from process V3 on (`finish::sharpen_view_at`): a = edge gradient, b = log
// luminance, c = the tone table; P[1] = Masking's threshold, P[2] = exposure.
@compute @workgroup_size(256)
fn sharpen_view_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let l = b[i];
    let y = GREY * exp2(l + pf(2u));
    let o = tone_c(y);
    let st = sharp_slope(y, tone_c(y * TONE_UP), tone_c(y * TONE_DOWN));
    dst[i] = sharp_mask(a[i], exp2(0.5 * l), sqrt(max(o, 0.0)), st, pf(1u));
}

// The Radius / Detail preview from process V3 on (`finish::sharpen_detail_at`): a = log luminance,
// b = the band, c = the tone table; P[1] = exposure, P[2] = the cap.
@compute @workgroup_size(256)
fn sharpen_detail_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let l = a[i];
    let y = GREY * exp2(l + pf(1u));
    let qd = sqrt(max(tone_c(y), 0.0));
    let st = sharp_slope(y, tone_c(y * TONE_UP), tone_c(y * TONE_DOWN));
    let limit = pf(2u);
    var v = 0.5;
    if (limit > 0.0) {
        v = clamp(0.5 + 0.5 * sharp_delta(b[i], l, qd, st, 1.0, limit, 1.0) / limit, 0.0, 1.0);
    }
    dst[i] = v;
}

// Every P[1]-th value of `a` (airlight sampling). P[0] = output count.
@compute @workgroup_size(256)
fn subsample(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    dst[i] = a[i * pu(1u)];
}

// `redeye::eye_pixel` (a: image). P[1] = width, P[2] = eye count, then EYE_WORDS per eye:
// cx, cy, r, darken, pet, has catchlight, catchlight x, y, r (output pixels).
@compute @workgroup_size(256)
fn redeye_k(@builtin(global_invocation_id) g: vec3<u32>, @builtin(num_workgroups) nw: vec3<u32>) {
    let i = lin_index(g, nw);
    if (i >= pu(0u)) {
        return;
    }
    let w = pu(1u);
    let x = f32(i % w) + 0.5;
    let y = f32(i / w) + 0.5;
    var c = rgb_a(i);
    for (var e = 0u; e < pu(2u); e++) {
        let o = 3u + e * EYE_WORDS;
        let ex = x - pf(o);
        let ey = y - pf(o + 1u);
        let d = sqrt(ex * ex + ey * ey) / pf(o + 2u);
        var m = 1.0 - sstep(1.0, 1.35, d);
        if (pf(o + 4u) != 0.0) {
            m = 1.0 - sstep(0.95, 1.2, d);
        }
        let darken = pf(o + 3u);
        if (m > 0.0) {
            if (pf(o + 4u) != 0.0) {
                let k = min(lum2020(c), 0.04) * (1.0 - 0.85 * darken);
                c = c + (vec3<f32>(k) - c) * m;
            } else {
                let red = (c.x - max(c.y, c.z)) / max(c.x, 1e-6);
                let a = m * sstep(0.25, 0.5, red);
                let n = 0.5 * (c.y + c.z) * (1.0 - 0.9 * darken);
                c = c + (vec3<f32>(n) - c) * a;
            }
        }
        if (pf(o + 5u) != 0.0) {
            let lx = x - pf(o + 6u);
            let ly = y - pf(o + 7u);
            let dc = sqrt(lx * lx + ly * ly) / pf(o + 8u);
            let mc = 1.0 - sstep(0.5, 1.0, dc);
            if (mc > 0.0) {
                c = c + (vec3<f32>(0.9) - c) * mc;
            }
        }
    }
    put_rgb(i, c);
}
