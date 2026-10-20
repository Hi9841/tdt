//! Matrix products for the speech model. Weights are row-major `[out][in]`,
//! as the model file stores them, so every output is a dot product of two
//! contiguous rows.

use rayon::prelude::*;

/// Output columns per parallel task.
const COLUMN_BLOCK: usize = 64;

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
    pub fn forward(&self, x: &[f32], rows: usize) -> Vec<f32> {
        let mut out = matmul(x, rows, self.in_features, &self.weight, self.out_features);
        if let Some(bias) = &self.bias {
            for row in out.chunks_exact_mut(self.out_features) {
                for (value, b) in row.iter_mut().zip(bias) {
                    *value += b;
                }
            }
        }
        out
    }
}

/// Ternary layer: `w = scale[row][col / group] * (code - 1)`, code in {0, 1, 2}.
pub struct Ternary {
    pub out_features: usize,
    pub in_features: usize,
    group: usize,
    /// `out x in`, one code per byte.
    codes: Vec<u8>,
    /// `out x (in / group)`.
    scales: Vec<f32>,
}

impl Ternary {
    /// `packed` holds 5 base-3 digits per byte, least significant first,
    /// `ceil(in / 5)` bytes per row.
    pub fn from_packed(
        packed: &[u8],
        scales: Vec<f32>,
        out_features: usize,
        in_features: usize,
        group: usize,
    ) -> Result<Self, String> {
        let row_bytes = in_features.div_ceil(5);
        if packed.len() != out_features * row_bytes
            || group == 0
            || in_features % group != 0
            || scales.len() != out_features * (in_features / group)
        {
            return Err(format!(
                "ternary layer {out_features}x{in_features} does not match its tensors"
            ));
        }
        let mut codes = vec![0u8; out_features * in_features];
        for (row, (packed_row, code_row)) in packed
            .chunks_exact(row_bytes)
            .zip(codes.chunks_exact_mut(in_features))
            .enumerate()
        {
            if packed_row.iter().any(|byte| *byte >= 243) {
                return Err(format!("ternary byte above 242 in row {row}"));
            }
            for (col, code) in code_row.iter_mut().enumerate() {
                *code = (packed_row[col / 5] / POW3[col % 5]) % 3;
            }
        }
        Ok(Self {
            out_features,
            in_features,
            group,
            codes,
            scales,
        })
    }

    /// `x` is `rows x in_features`; the result is `rows x out_features`.
    pub fn forward(&self, x: &[f32], rows: usize) -> Vec<f32> {
        let (k, n, group) = (self.in_features, self.out_features, self.group);
        let groups = k / group;
        let mut out = vec![0.0f32; rows * n];
        let out_ptr = SendPtr(out.as_mut_ptr());
        (0..n.div_ceil(COLUMN_BLOCK))
            .into_par_iter()
            .for_each(|block| {
                let columns = block * COLUMN_BLOCK..((block + 1) * COLUMN_BLOCK).min(n);
                for col in columns {
                    let codes = &self.codes[col * k..(col + 1) * k];
                    let scales = &self.scales[col * groups..(col + 1) * groups];
                    for row in 0..rows {
                        let xr = &x[row * k..(row + 1) * k];
                        let mut total = 0.0f32;
                        for g in 0..groups {
                            let span = g * group..(g + 1) * group;
                            let mut sum = 0.0f32;
                            for (xv, code) in xr[span.clone()].iter().zip(&codes[span]) {
                                sum += xv * (f32::from(*code) - 1.0);
                            }
                            total += sum * scales[g];
                        }
                        // SAFETY: each (row, col) is written by exactly one task.
                        unsafe { *out_ptr.get().add(row * n + col) = total };
                    }
                }
            });
        out
    }
}

const POW3: [u8; 5] = [1, 3, 9, 27, 81];

#[derive(Clone, Copy)]
struct SendPtr(*mut f32);
// SAFETY: tasks write disjoint elements; the buffer outlives the parallel scope.
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}
impl SendPtr {
    fn get(self) -> *mut f32 {
        self.0
    }
}

/// `out[m][n] = sum_k x[m][k] * w[n][k]`.
pub fn matmul(x: &[f32], rows: usize, k: usize, w: &[f32], n: usize) -> Vec<f32> {
    debug_assert_eq!(x.len(), rows * k);
    debug_assert_eq!(w.len(), n * k);
    let mut out = vec![0.0f32; rows * n];
    let out_ptr = SendPtr(out.as_mut_ptr());
    (0..n.div_ceil(COLUMN_BLOCK))
        .into_par_iter()
        .for_each(|block| {
            let columns = block * COLUMN_BLOCK..((block + 1) * COLUMN_BLOCK).min(n);
            for col in columns {
                let wr = &w[col * k..(col + 1) * k];
                for row in 0..rows {
                    let value = dot(&x[row * k..(row + 1) * k], wr);
                    // SAFETY: each (row, col) is written by exactly one task.
                    unsafe { *out_ptr.get().add(row * n + col) = value };
                }
            }
        });
    out
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 16];
    let chunks = a.len() / 16;
    for (ca, cb) in a.chunks_exact(16).zip(b.chunks_exact(16)) {
        for i in 0..16 {
            lanes[i] += ca[i] * cb[i];
        }
    }
    let mut total: f32 = lanes.iter().sum();
    for i in chunks * 16..a.len() {
        total += a[i] * b[i];
    }
    total
}

#[cfg(test)]
mod tests {
    use super::{matmul, Dense, Ternary};

    #[test]
    fn packed_ternary_rows_decode_and_multiply() {
        // Codes 2,0,1,2,1 | 0,2 are weights +1,-1,0,+1,0 | -1,+1.
        // Byte 0 = 2 + 0*3 + 1*9 + 2*27 + 1*81 = 146, byte 1 = 0 + 2*3 = 6.
        let layer = Ternary::from_packed(&[146, 6], vec![2.0], 1, 7, 7).expect("layer");
        let x = [1.0, 10.0, 100.0, 1000.0, 5.0, 3.0, 7.0];
        assert_eq!(
            layer.forward(&x, 1),
            vec![2.0 * (1.0 - 10.0 + 1000.0 - 3.0 + 7.0)]
        );
        assert!(Ternary::from_packed(&[243, 6], vec![2.0], 1, 7, 7).is_err());
    }

    #[test]
    fn dense_forward_adds_bias_per_column() {
        let layer = Dense::new(
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
            Some(vec![0.5, -1.0]),
            2,
            3,
        )
        .expect("layer");
        let y = layer.forward(&[1.0, 0.0, 2.0, 0.0, 1.0, 0.0], 2);
        assert_eq!(y, vec![7.5, 15.0, 2.5, 4.0]);
        let big: Vec<f32> = (0..70 * 33).map(|i| (i % 7) as f32).collect();
        let x: Vec<f32> = (0..2 * 33).map(|i| (i % 3) as f32).collect();
        let out = matmul(&x, 2, 33, &big, 70);
        let expected: f32 = (0..33).map(|k| x[33 + k] * big[69 * 33 + k]).sum();
        assert_eq!(out[70 + 69], expected);
    }
}
