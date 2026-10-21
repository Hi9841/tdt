//! Matrix-vector product with f16 weights, for the decoder. Each decoding
//! step reads every weight once, so the step is bound by memory traffic;
//! keeping the weights as stored (f16) halves it at no loss of precision.

use super::buf::Buf;
use super::team;
use super::tensors::f16_to_f32;
use std::sync::OnceLock;

/// Output rows per parallel task.
const ROWS_PER_TASK: usize = 128;

pub struct HalfDense {
    pub out_features: usize,
    pub in_features: usize,
    weight: Vec<u16>,
    bias: Option<Vec<f32>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx512,
}

fn level() -> Level {
    static LEVEL: OnceLock<Level> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        #[cfg(target_arch = "x86_64")]
        {
            let forced = std::env::var("TDT_NATIVE_ISA").unwrap_or_default();
            let avx2 = is_x86_feature_detected!("avx2")
                && is_x86_feature_detected!("fma")
                && is_x86_feature_detected!("f16c");
            if forced == "scalar" {
                return Level::Scalar;
            }
            if forced != "avx2" && is_x86_feature_detected!("avx512f") {
                return Level::Avx512;
            }
            if avx2 {
                return Level::Avx2;
            }
        }
        Level::Scalar
    })
}

impl HalfDense {
    /// `weight` is row-major `out x in` f16 bits.
    pub fn new(
        weight: Vec<u16>,
        bias: Option<Vec<f32>>,
        out_features: usize,
        in_features: usize,
    ) -> Result<Self, String> {
        if weight.len() != out_features * in_features
            || bias.as_ref().is_some_and(|b| b.len() != out_features)
        {
            return Err(format!(
                "f16 layer {out_features}x{in_features} has {} weights",
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

    /// Rows of `a` followed by rows of `b`, side by side: `[a | b]`, so one
    /// product covers `a * x + b * y` for the input `[x | y]`.
    pub fn side_by_side(
        a: &[u16],
        b: &[u16],
        rows: usize,
        a_cols: usize,
        b_cols: usize,
    ) -> Vec<u16> {
        let mut out = Vec::with_capacity(rows * (a_cols + b_cols));
        for r in 0..rows {
            out.extend_from_slice(&a[r * a_cols..(r + 1) * a_cols]);
            out.extend_from_slice(&b[r * b_cols..(r + 1) * b_cols]);
        }
        out
    }

    /// One input vector of `in_features` values.
    pub fn matvec(&self, x: &[f32]) -> Buf {
        assert_eq!(x.len(), self.in_features);
        let (n, k) = (self.out_features, self.in_features);
        let mut out = Buf::scratch(n);
        let level = level();
        team::chunks_mut(&mut out, ROWS_PER_TASK, |task, rows| {
            let first = task * ROWS_PER_TASK;
            let weights = &self.weight[first * k..(first + rows.len()) * k];
            match level {
                #[cfg(target_arch = "x86_64")]
                // SAFETY: the level was detected from the CPU.
                Level::Avx512 => unsafe { avx512_rows(weights, k, x, rows) },
                #[cfg(target_arch = "x86_64")]
                // SAFETY: the level was detected from the CPU.
                Level::Avx2 => unsafe { avx2_rows(weights, k, x, rows) },
                Level::Scalar => scalar_rows(weights, k, x, rows),
            }
            if let Some(bias) = &self.bias {
                for (o, b) in rows.iter_mut().zip(&bias[first..]) {
                    *o += b;
                }
            }
        });
        out
    }
}

fn scalar_rows(weights: &[u16], k: usize, x: &[f32], out: &mut [f32]) {
    for (row, o) in weights.chunks_exact(k).zip(out.iter_mut()) {
        *o = row.iter().zip(x).map(|(w, v)| f16_to_f32(*w) * v).sum();
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
unsafe fn avx512_rows(weights: &[u16], k: usize, x: &[f32], out: &mut [f32]) {
    use std::arch::x86_64::*;
    for (row, o) in weights.chunks_exact(k).zip(out.iter_mut()) {
        let mut acc = _mm512_setzero_ps();
        let mut i = 0;
        while i + 16 <= k {
            let w = _mm512_cvtph_ps(_mm256_loadu_si256(row.as_ptr().add(i).cast()));
            acc = _mm512_fmadd_ps(w, _mm512_loadu_ps(x.as_ptr().add(i)), acc);
            i += 16;
        }
        let mut total = _mm512_reduce_add_ps(acc);
        for j in i..k {
            total += f16_to_f32(row[j]) * x[j];
        }
        *o = total;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma,f16c")]
unsafe fn avx2_rows(weights: &[u16], k: usize, x: &[f32], out: &mut [f32]) {
    use std::arch::x86_64::*;
    for (row, o) in weights.chunks_exact(k).zip(out.iter_mut()) {
        let mut acc = _mm256_setzero_ps();
        let mut i = 0;
        while i + 8 <= k {
            let w = _mm256_cvtph_ps(_mm_loadu_si128(row.as_ptr().add(i).cast()));
            acc = _mm256_fmadd_ps(w, _mm256_loadu_ps(x.as_ptr().add(i)), acc);
            i += 8;
        }
        let mut lanes = [0.0f32; 8];
        _mm256_storeu_ps(lanes.as_mut_ptr(), acc);
        let mut total: f32 = lanes.iter().sum();
        for j in i..k {
            total += f16_to_f32(row[j]) * x[j];
        }
        *o = total;
    }
}

#[cfg(test)]
mod tests {
    use super::HalfDense;
    use crate::stt::native::tensors::f16_to_f32;

    #[test]
    fn f16_rows_multiply_like_their_f32_values() {
        let (n, k) = (300, 37);
        // Halves between 0.5 and 2 with both signs.
        let weight: Vec<u16> = (0..n * k)
            .map(|i| (0x3800 + (i % 1024) as u16) | ((i % 2) << 15) as u16)
            .collect();
        let bias: Vec<f32> = (0..n).map(|i| i as f32 * 0.01).collect();
        let x: Vec<f32> = (0..k).map(|i| (i as f32 - 18.0) * 0.1).collect();
        let layer = HalfDense::new(weight.clone(), Some(bias.clone()), n, k).expect("layer");
        let out = layer.matvec(&x);
        for r in 0..n {
            let expected: f32 = (0..k)
                .map(|c| f16_to_f32(weight[r * k + c]) * x[c])
                .sum::<f32>()
                + bias[r];
            assert!(
                (out[r] - expected).abs() <= 1e-4 * expected.abs().max(1.0),
                "{r}: {} vs {expected}",
                out[r]
            );
        }
    }

    #[test]
    fn side_by_side_rows_concatenate() {
        let joined = HalfDense::side_by_side(&[1, 2, 3, 4], &[9, 8], 2, 2, 1);
        assert_eq!(joined, vec![1, 2, 9, 3, 4, 8]);
    }
}
