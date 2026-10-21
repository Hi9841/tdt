//! Matrix products for the speech model. Weights are row-major `[out][in]`,
//! as the model file stores them, so every output is a dot product of two
//! contiguous rows.

use super::buf::Buf;
use super::fgemm;
use super::team;

/// Output rows per task of the bias and ReLU pass.
const EPILOGUE_ROWS: usize = 64;

/// Dense layer with f32 weights.
pub struct Dense {
    pub out_features: usize,
    pub in_features: usize,
    weight: Vec<f32>,
    bias: Option<Vec<f32>>,
}

impl Dense {
    pub fn new(
        weight: Vec<f32>,
        bias: Option<Vec<f32>>,
        out_features: usize,
        in_features: usize,
    ) -> Result<Self, String> {
        if weight.len() != out_features * in_features
            || bias.as_ref().is_some_and(|b| b.len() != out_features)
        {
            return Err(format!(
                "dense layer {out_features}x{in_features} has {} weights",
                weight.len()
            ));
        }
        Ok(Self {
            out_features,
            in_features,
            weight,
            bias,
        })
    }

    /// `x` is `rows x in_features`; the result is `rows x out_features`.
    pub fn forward(&self, x: &[f32], rows: usize) -> Buf {
        self.forward_then(x, rows, false)
    }

    /// `forward`, then `max(0, v)` on every output.
    pub fn forward_relu(&self, x: &[f32], rows: usize) -> Buf {
        self.forward_then(x, rows, true)
    }

    fn forward_then(&self, x: &[f32], rows: usize, relu: bool) -> Buf {
        let n = self.out_features;
        let mut out = fgemm::matmul(x, rows, self.in_features, &self.weight, n);
        if self.bias.is_none() && !relu {
            return out;
        }
        team::chunks_mut(&mut out, EPILOGUE_ROWS * n, |_, block| {
            for row in block.chunks_exact_mut(n) {
                if let Some(bias) = &self.bias {
                    for (value, b) in row.iter_mut().zip(bias) {
                        *value += b;
                    }
                }
                if relu {
                    row.iter_mut().for_each(|v| *v = v.max(0.0));
                }
            }
        });
        out
    }
}
