//! MoGe-2's convolutional decoders: a five-level stack that starts at the token grid and doubles
//! the resolution four times (three transposed convolutions, then a bilinear upsample), with a
//! residual block on the middle levels. The shared neck and each head (normals, points, mask) are
//! one such stack; a head reads the neck's five outputs and its last level is projected to the
//! head's channels.
//!
//! Modified work (MIT License): written by the LightCraft contributors in 2026 after the
//! Python/PyTorch MoGe code (`moge/model/modules/conv_stack.py`), Copyright (c) Microsoft
//! Corporation; translated to Rust on candle and reduced to the configuration of the
//! `moge-2-vits-normal` checkpoint (ReLU, no normalization layers). See NOTICE.

use candle_core::{Module, Tensor};
use candle_nn::{Conv2d, Conv2dConfig, ConvTranspose2d, ConvTranspose2dConfig, VarBuilder};

use crate::Result;

/// Channels of the five levels.
pub const DIMS: [usize; 5] = [384, 256, 128, 64, 32];
/// Residual blocks per level.
const RES_BLOCKS: [usize; 5] = [0, 1, 1, 1, 0];

/// A 3 × 3 convolution whose border repeats the edge pixels (PyTorch's `padding_mode="replicate"`).
struct Conv3(Conv2d);

impl Conv3 {
    fn new(cin: usize, cout: usize, vb: VarBuilder) -> Result<Self> {
        Ok(Self(candle_nn::conv2d(cin, cout, 3, Conv2dConfig::default(), vb)?))
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(self.0.forward(&x.pad_with_same(2, 1, 1)?.pad_with_same(3, 1, 1)?)?)
    }
}

fn conv1(cin: usize, cout: usize, vb: VarBuilder) -> Result<Conv2d> {
    Ok(candle_nn::conv2d(cin, cout, 1, Conv2dConfig::default(), vb)?)
}

/// `x + conv(relu(conv(relu(x))))`.
struct ResBlock {
    a: Conv3,
    b: Conv3,
}

impl ResBlock {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let y = self.b.forward(&self.a.forward(&x.relu()?)?.relu()?)?;
        Ok((x + y)?)
    }
}

/// Doubles the resolution on the way from one level to the next.
enum Resampler {
    Transposed(ConvTranspose2d, Conv3),
    Bilinear(Conv3),
}

impl Resampler {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        match self {
            Resampler::Transposed(up, conv) => conv.forward(&up.forward(x)?),
            Resampler::Bilinear(conv) => {
                let (_, _, h, w) = x.dims4()?;
                conv.forward(&x.upsample_bilinear2d(h * 2, w * 2, false)?)
            }
        }
    }
}

pub struct Stack {
    inputs: Vec<Conv2d>,
    blocks: Vec<Vec<ResBlock>>,
    resamplers: Vec<Resampler>,
    /// The last level's projection (a head); `None` for the neck, whose outputs are its levels.
    output: Option<Conv2d>,
}

impl Stack {
    /// A stack reading `dim_in[i]` channels at level `i`, with the last level projected to
    /// `out` channels when given.
    pub fn new(dim_in: [usize; 5], out: Option<usize>, vb: VarBuilder) -> Result<Self> {
        let inputs = (0..5).map(|i| conv1(dim_in[i], DIMS[i], vb.pp(format!("input_blocks.{i}")))).collect::<Result<Vec<_>>>()?;
        let mut blocks = Vec::with_capacity(5);
        for (i, (dim, count)) in DIMS.into_iter().zip(RES_BLOCKS).enumerate() {
            let level = (0..count)
                .map(|j| {
                    let vb = vb.pp(format!("res_blocks.{i}.{j}.layers"));
                    Ok(ResBlock { a: Conv3::new(dim, dim, vb.pp("2"))?, b: Conv3::new(dim, dim, vb.pp("5"))? })
                })
                .collect::<Result<Vec<_>>>()?;
            blocks.push(level);
        }
        let mut resamplers = Vec::with_capacity(4);
        for i in 0..4 {
            let (from, to) = (DIMS[i], DIMS[i + 1]);
            let vb = vb.pp(format!("resamplers.{i}"));
            resamplers.push(if i < 3 {
                let config = ConvTranspose2dConfig { stride: 2, ..Default::default() };
                Resampler::Transposed(candle_nn::conv_transpose2d(from, to, 2, config, vb.pp("0"))?, Conv3::new(to, to, vb.pp("1"))?)
            } else {
                Resampler::Bilinear(Conv3::new(from, to, vb.pp("1"))?)
            });
        }
        let output = match out {
            Some(channels) => Some(conv1(DIMS[4], channels, vb.pp("output_blocks.4"))?),
            None => None,
        };
        Ok(Self { inputs, blocks, resamplers, output })
    }

    /// Run the stack on one input per level; returns every level's features (the last one
    /// projected, for a head).
    pub fn forward(&self, features: &[Tensor]) -> Result<Vec<Tensor>> {
        let mut out = Vec::with_capacity(5);
        let mut x: Option<Tensor> = None;
        for (i, ((input, feature), blocks)) in self.inputs.iter().zip(features).zip(&self.blocks).enumerate() {
            let feature = input.forward(feature)?;
            let mut level = match x.take() {
                Some(previous) => (previous + feature)?,
                None => feature,
            };
            for block in blocks {
                level = block.forward(&level)?;
            }
            let last = i + 1 == self.inputs.len();
            out.push(match (&self.output, last) {
                (Some(project), true) => project.forward(&level)?,
                _ => level.clone(),
            });
            if let Some(resampler) = self.resamplers.get(i) {
                x = Some(resampler.forward(&level)?);
            }
        }
        Ok(out)
    }
}
