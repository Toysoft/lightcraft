//! JPEG XL decode via `jxl-oxide` (pure Rust). Enum-encoded images are rendered straight into linear
//! Rec.2020; ICC-tagged ones are rendered in their own space and interpreted with our ICC path.
//! XYB (lossy) images are rendered to linear sRGB primaries and converted to Rec.2020 here:
//! jxl-oxide 0.12 desaturates colours outside sRGB when it renders XYB to wider primaries.
//! jxl-oxide applies the codestream orientation itself, so `orientation` is reported as 1.

use crate::convert::{Buf, Meta, Model, Raw, check_size, finish};
use crate::space::{NamedSpace, SourceSpace, SpaceOrigin, Trc};
use crate::{DecodeOptions, Decoded, Error, Format, Result};
use jxl_oxide::color::{ColourSpace, EnumColourEncoding, Primaries, RenderingIntent, TransferFunction, WhitePoint};
use jxl_oxide::{AllocTracker, AuxBoxData, InitializeResult, JxlImage};

const F: Format = Format::Jxl;
const DECODER_BYTES_PER_PIXEL: usize = 16 * 4;
const DECODER_MARGIN_BYTES: usize = 64 << 10;

fn decoder_budget(max_pixels: u64) -> usize {
    usize::try_from(max_pixels).unwrap_or(usize::MAX).saturating_mul(DECODER_BYTES_PER_PIXEL).saturating_add(DECODER_MARGIN_BYTES)
}

fn shift_buffer(buf: &mut [u8], valid: &mut usize, consumed: usize) -> Result<()> {
    let remaining = valid.checked_sub(consumed).ok_or_else(|| err("invalid JPEG XL parser offset"))?;
    let tail = buf.get(consumed..*valid).ok_or_else(|| err("invalid JPEG XL parser offset"))?.to_vec();
    buf.get_mut(..remaining).ok_or_else(|| err("invalid JPEG XL parser offset"))?.copy_from_slice(&tail);
    *valid = remaining;
    Ok(())
}

fn load(bytes: &[u8], opts: &DecodeOptions) -> Result<JxlImage> {
    let tracker = AllocTracker::with_limit(decoder_budget(opts.max_pixels));
    let mut uninit = JxlImage::builder().alloc_tracker(tracker).build_uninit();
    let mut buf = [0u8; 4096];
    let (mut pos, mut valid) = (0usize, 0usize);
    let mut img = loop {
        if valid < buf.len() {
            let count = (buf.len() - valid).min(bytes.len().saturating_sub(pos));
            if count == 0 {
                return Err(err("truncated JPEG XL image header"));
            }
            let source = bytes.get(pos..pos + count).ok_or_else(|| err("invalid JPEG XL input offset"))?;
            buf.get_mut(valid..valid + count).ok_or_else(|| err("invalid JPEG XL buffer offset"))?.copy_from_slice(source);
            pos += count;
            valid += count;
        }
        let consumed = uninit.feed_bytes(buf.get(..valid).ok_or_else(|| err("invalid JPEG XL buffer length"))?).map_err(err)?;
        shift_buffer(&mut buf, &mut valid, consumed)?;
        match uninit.try_init().map_err(err)? {
            InitializeResult::NeedMoreData(next) => uninit = next,
            InitializeResult::Initialized(image) => break image,
        }
    };
    check_size(F, img.width() as u64, img.height() as u64, opts)?;

    while pos < bytes.len() || valid > 0 {
        if valid < buf.len() && pos < bytes.len() {
            let count = (buf.len() - valid).min(bytes.len() - pos);
            let source = bytes.get(pos..pos + count).ok_or_else(|| err("invalid JPEG XL input offset"))?;
            buf.get_mut(valid..valid + count).ok_or_else(|| err("invalid JPEG XL buffer offset"))?.copy_from_slice(source);
            pos += count;
            valid += count;
        }
        let consumed = img.feed_bytes(buf.get(..valid).ok_or_else(|| err("invalid JPEG XL buffer length"))?).map_err(err)?;
        if consumed == 0 && pos == bytes.len() {
            return Err(err("truncated JPEG XL image"));
        }
        shift_buffer(&mut buf, &mut valid, consumed)?;
    }
    img.finalize().map_err(err)?;
    Ok(img)
}

fn err(e: impl std::fmt::Display) -> Error {
    Error::Malformed(F, e.to_string())
}

pub(crate) fn decode(bytes: &[u8], opts: &DecodeOptions) -> Result<Decoded> {
    let mut img = load(bytes, opts)?;
    let (w, h) = (img.width(), img.height());
    check_size(F, w as u64, h as u64, opts)?;
    let header = img.image_header();
    let bits = header.metadata.bit_depth.bits_per_sample() as u8;
    let float = matches!(header.metadata.bit_depth, jxl_oxide::image::BitDepth::FloatSample { .. });
    let gray = header.metadata.grayscale();
    let icc = img.original_icc().map(|v| v.to_vec());
    let mut meta = Meta { orientation: Some(1), ..Default::default() };
    // linear sRGB primaries, unclamped, then our own matrix to Rec.2020 (see the module docs)
    let via_srgb = icc.is_none() && !gray && header.metadata.xyb_encoded;
    if icc.is_none() {
        img.request_color_encoding(EnumColourEncoding {
            colour_space: if gray { ColourSpace::Grey } else { ColourSpace::Rgb },
            white_point: WhitePoint::D65,
            primaries: if via_srgb { Primaries::Srgb } else { Primaries::Bt2100 },
            tf: TransferFunction::Linear,
            rendering_intent: RenderingIntent::Relative,
        });
        let mut s = SourceSpace::named(NamedSpace::Rec2020, SpaceOrigin::Container);
        s.trc = Some([Trc::Linear, Trc::Linear, Trc::Linear]);
        meta.already_linear = Some(s);
    } else {
        meta.icc = icc;
    }
    if let Ok(AuxBoxData::Data(ex)) = img.aux_boxes().first_exif() {
        meta.exif = Some(ex.payload().to_vec());
    }
    if let AuxBoxData::Data(x) = img.aux_boxes().first_xml() {
        meta.xmp = Some(String::from_utf8_lossy(x).trim_end_matches('\0').to_string());
    }
    let render = img.render_frame(0).map_err(err)?;
    let fb = render.image_all_channels();
    let (fw, fh, ch) = (fb.width(), fb.height(), fb.channels());
    let n_color = if gray { 1 } else { 3 };
    let has_alpha = img.pixel_format().has_alpha();
    if ch < n_color {
        return Err(err("too few channels"));
    }
    let keep = n_color + has_alpha as usize;
    let src = fb.buf();
    let mut v = Vec::with_capacity(fw * fh * keep);
    let to_2020 = lightcraft_color::SRGB.to_space(&lightcraft_color::REC2020).to_f32();
    for px in src.chunks_exact(ch) {
        let start = v.len();
        v.extend_from_slice(&px[..keep.min(ch)]);
        if keep > ch {
            v.push(1.0);
        }
        if via_srgb && let Some(rgb) = v.get_mut(start..start + 3) {
            let [r, g, b] = [rgb[0], rgb[1], rgb[2]];
            for (o, m) in rgb.iter_mut().zip(&to_2020) {
                *o = m[0] * r + m[1] * g + m[2] * b;
            }
        }
    }
    let raw = Raw {
        width: fw,
        height: fh,
        model: if gray { Model::Gray } else { Model::Rgb },
        alpha: has_alpha,
        premultiplied: false,
        buf: Buf::F32(v),
        bit_depth: bits,
    };
    let mut d = finish(F, raw, meta, (w, h), opts)?;
    d.float = float;
    Ok(d)
}
