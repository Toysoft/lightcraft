//! The DINOv2 ViT-S/14 vision backbone MoGe-2 encodes the photo with: 12 pre-norm transformer
//! blocks with layer scale, a class token, and learned position embeddings resampled (bicubic)
//! to the token grid in use.
//!
//! Modified work (Apache License 2.0, §4(b)): written by the LightCraft contributors in 2026 after
//! the Python/PyTorch DINOv2 code (`dinov2/models/vision_transformer.py`), Copyright Meta
//! Platforms, Inc. and affiliates; translated to Rust on candle and reduced to inference of the
//! ViT-S/14 configuration. See NOTICE.

use candle_core::{Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, LayerNorm, Linear, VarBuilder};

use crate::{Error, Result};

pub const PATCH: usize = 14;
pub const HIDDEN: usize = 384;
const HEADS: usize = 6;
const LAYERS: usize = 12;
/// Side of the grid the position embeddings were trained on (518 / 14).
const PRETRAIN_GRID: usize = 37;
const EPS: f64 = 1e-6;
/// DINOv2 nudges the resampling scale of the position embeddings by this much of a token.
const INTERPOLATE_OFFSET: f64 = 0.1;

struct Block {
    norm1: LayerNorm,
    qkv: Linear,
    proj: Linear,
    scale1: Tensor,
    norm2: LayerNorm,
    fc1: Linear,
    fc2: Linear,
    scale2: Tensor,
}

impl Block {
    fn new(vb: VarBuilder) -> Result<Self> {
        Ok(Self {
            norm1: candle_nn::layer_norm(HIDDEN, EPS, vb.pp("norm1"))?,
            qkv: candle_nn::linear(HIDDEN, HIDDEN * 3, vb.pp("attn.qkv"))?,
            proj: candle_nn::linear(HIDDEN, HIDDEN, vb.pp("attn.proj"))?,
            scale1: vb.get(HIDDEN, "ls1.gamma")?,
            norm2: candle_nn::layer_norm(HIDDEN, EPS, vb.pp("norm2"))?,
            fc1: candle_nn::linear(HIDDEN, HIDDEN * 4, vb.pp("mlp.fc1"))?,
            fc2: candle_nn::linear(HIDDEN * 4, HIDDEN, vb.pp("mlp.fc2"))?,
            scale2: vb.get(HIDDEN, "ls2.gamma")?,
        })
    }

    fn attention(&self, x: &Tensor) -> Result<Tensor> {
        let (b, len, _) = x.dims3()?;
        let head_dim = HIDDEN / HEADS;
        // [b, len, 3 · hidden] → three [b, heads, len, head_dim]
        let qkv = self.qkv.forward(x)?.reshape((b, len, 3, HEADS, head_dim))?.permute((2, 0, 3, 1, 4))?;
        let (q, k, v) = (qkv.get(0)?, qkv.get(1)?, qkv.get(2)?);
        let scores = (q.matmul(&k.t()?)? * (head_dim as f64).powf(-0.5))?;
        let mixed = candle_nn::ops::softmax_last_dim(&scores)?.matmul(&v)?;
        Ok(self.proj.forward(&mixed.transpose(1, 2)?.reshape((b, len, HIDDEN))?)?)
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let x = (x + self.attention(&self.norm1.forward(x)?)?.broadcast_mul(&self.scale1)?)?;
        let mlp = self.fc2.forward(&self.fc1.forward(&self.norm2.forward(&x)?)?.gelu_erf()?)?;
        Ok((&x + mlp.broadcast_mul(&self.scale2)?)?)
    }
}

pub struct Vit {
    patch: Conv2d,
    class: Tensor,
    /// Position embedding of the class token `[hidden]`, and of the trained grid, row-major `[37 · 37 · hidden]`.
    class_position: Vec<f32>,
    grid_position: Vec<f32>,
    blocks: Vec<Block>,
    norm: LayerNorm,
}

/// The cubic convolution kernel PyTorch's bicubic interpolation uses (a = −0.75).
fn cubic(t: f64) -> f64 {
    const A: f64 = -0.75;
    let t = t.abs();
    if t <= 1.0 {
        ((A + 2.0) * t - (A + 3.0)) * t * t + 1.0
    } else if t < 2.0 {
        ((A * t - 5.0 * A) * t + 8.0 * A) * t - 4.0 * A
    } else {
        0.0
    }
}

/// For each of `out` positions along an axis of `PRETRAIN_GRID` samples scaled by `scale`: the
/// four source indices (edges repeated) and their weights.
fn cubic_taps(out: usize, scale: f64) -> Vec<[(usize, f32); 4]> {
    (0..out)
        .map(|i| {
            let src = (i as f64 + 0.5) / scale - 0.5;
            let base = src.floor();
            let t = src - base;
            std::array::from_fn(|k| {
                let offset = k as f64 - 1.0;
                let index = (base + offset).clamp(0.0, (PRETRAIN_GRID - 1) as f64) as usize;
                (index, cubic(offset - t) as f32)
            })
        })
        .collect()
}

impl Vit {
    pub fn new(vb: VarBuilder) -> Result<Self> {
        let patch = candle_nn::conv2d(3, HIDDEN, PATCH, Conv2dConfig { stride: PATCH, ..Default::default() }, vb.pp("patch_embed.proj"))?;
        let position: Vec<f32> = vb.get((1, 1 + PRETRAIN_GRID * PRETRAIN_GRID, HIDDEN), "pos_embed")?.flatten_all()?.to_vec1()?;
        let (class_position, grid_position) =
            position.split_at_checked(HIDDEN).ok_or_else(|| Error::Model("position embeddings too short".into()))?;
        let blocks = (0..LAYERS).map(|i| Block::new(vb.pp(format!("blocks.{i}")))).collect::<Result<Vec<_>>>()?;
        Ok(Self {
            patch,
            class: vb.get((1, 1, HIDDEN), "cls_token")?,
            class_position: class_position.to_vec(),
            grid_position: grid_position.to_vec(),
            blocks,
            norm: candle_nn::layer_norm(HIDDEN, EPS, vb.pp("norm"))?,
        })
    }

    /// The position embeddings for a `rows × cols` token grid, `[1, 1 + rows · cols, hidden]`.
    fn positions(&self, rows: usize, cols: usize, like: &Tensor) -> Result<Tensor> {
        let scale = |n: usize| (n as f64 + INTERPOLATE_OFFSET) / PRETRAIN_GRID as f64;
        let (ys, xs) = (cubic_taps(rows, scale(rows)), cubic_taps(cols, scale(cols)));
        let mut out = Vec::with_capacity((1 + rows * cols) * HIDDEN);
        out.extend_from_slice(&self.class_position);
        for y_taps in &ys {
            for x_taps in &xs {
                let start = out.len();
                out.resize(start + HIDDEN, 0.0);
                for (sy, wy) in y_taps {
                    for (sx, wx) in x_taps {
                        let at = (sy * PRETRAIN_GRID + sx) * HIDDEN;
                        let (Some(src), Some(dst)) = (self.grid_position.get(at..at + HIDDEN), out.get_mut(start..start + HIDDEN)) else {
                            return Err(Error::Model("position embeddings out of range".into()));
                        };
                        let w = wy * wx;
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d += w * s;
                        }
                    }
                }
            }
        }
        Ok(Tensor::from_vec(out, (1, 1 + rows * cols, HIDDEN), like.device())?)
    }

    /// Run `image` (`[1, 3, rows · 14, cols · 14]`, normalized) through the backbone and return,
    /// for each block in `take`, its normalized patch tokens as `[1, hidden, rows, cols]`.
    pub fn features(&self, image: &Tensor, rows: usize, cols: usize, take: &[usize]) -> Result<Vec<Tensor>> {
        let tokens = self.patch.forward(image)?.flatten_from(2)?.transpose(1, 2)?;
        let mut x = (Tensor::cat(&[&self.class, &tokens], 1)? + self.positions(rows, cols, image)?)?;
        let mut out = Vec::with_capacity(take.len());
        for (i, block) in self.blocks.iter().enumerate() {
            x = block.forward(&x)?;
            if take.contains(&i) {
                let patches = self.norm.forward(&x)?.narrow(1, 1, rows * cols)?;
                out.push(patches.transpose(1, 2)?.reshape((1, HIDDEN, rows, cols))?.contiguous()?);
            }
        }
        if out.len() != take.len() {
            return Err(Error::Model("a requested backbone layer does not exist".into()));
        }
        Ok(out)
    }
}
