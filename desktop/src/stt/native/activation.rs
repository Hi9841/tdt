//! Element-wise exp, SiLU, and sigmoid on whole slices. The Windows target
//! has no vector math library, so `f32::exp` runs one value at a time. These
//! run one polynomial (Cephes `expf`, within about 2 ulp) on 16 or 8 lanes,
//! and the scalar path runs the same polynomial.

use super::fgemm::{level, Level};

const LOG2E: f32 = std::f32::consts::LOG2_E;
/// ln 2 split in two, so `n * LN2_HI` is exact for every `n` in range.
const LN2_HI: f32 = 0.693_359_4;
const LN2_LO: f32 = -2.121_944_4e-4;
/// `exp` of these stays a normal f32, and `n` stays in -126..=127.
const LOW: f32 = -87.3;
const HIGH: f32 = 88.0;
const POLY: [f32; 6] = [
    1.987_569_1e-4,
    1.398_199_9e-3,
    8.333_452e-3,
    4.166_579_6e-2,
    0.166_666_65,
    0.5,
];

#[derive(Clone, Copy)]
enum Op {
    Exp,
    Silu,
    Sigmoid,
}

/// `v = e^v`.
pub fn exp(values: &mut [f32]) {
    apply(values, Op::Exp);
}

/// `v = v / (1 + e^-v)`.
pub fn silu(values: &mut [f32]) {
    apply(values, Op::Silu);
}

/// `v = 1 / (1 + e^-v)`.
pub fn sigmoid(values: &mut [f32]) {
    apply(values, Op::Sigmoid);
}

fn apply(values: &mut [f32], op: Op) {
    match level() {
        #[cfg(target_arch = "x86_64")]
        // SAFETY: the level was detected from the CPU.
        Level::Avx512 => unsafe { avx512::apply(values, op) },
        #[cfg(target_arch = "x86_64")]
        // SAFETY: the level was detected from the CPU.
        Level::Avx2 => unsafe { avx2::apply(values, op) },
        Level::Scalar => values.iter_mut().for_each(|v| *v = scalar(*v, op)),
    }
}

fn scalar(v: f32, op: Op) -> f32 {
    match op {
        Op::Exp => exp_scalar(v),
        Op::Silu => v / (1.0 + exp_scalar(-v)),
        Op::Sigmoid => 1.0 / (1.0 + exp_scalar(-v)),
    }
}

fn exp_scalar(x: f32) -> f32 {
    let x = x.clamp(LOW, HIGH);
    let n = (x * LOG2E).round_ties_even();
    let r = x - n * LN2_HI - n * LN2_LO;
    let p = POLY.iter().fold(0.0f32, |p, c| p * r + c);
    let two_n = f32::from_bits(((n as i32 + 127) as u32) << 23);
    (p * r * r + r + 1.0) * two_n
}

#[cfg(target_arch = "x86_64")]
mod avx512 {
    use super::{Op, HIGH, LN2_HI, LN2_LO, LOG2E, LOW, POLY};
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx512f")]
    pub unsafe fn apply(values: &mut [f32], op: Op) {
        let (chunks, tail) = values.as_chunks_mut::<16>();
        for chunk in chunks {
            let v = _mm512_loadu_ps(chunk.as_ptr());
            _mm512_storeu_ps(chunk.as_mut_ptr(), compute(v, op));
        }
        if !tail.is_empty() {
            let mask: __mmask16 = ((1u32 << tail.len()) - 1) as u16;
            let v = _mm512_maskz_loadu_ps(mask, tail.as_ptr());
            _mm512_mask_storeu_ps(tail.as_mut_ptr(), mask, compute(v, op));
        }
    }

    #[target_feature(enable = "avx512f")]
    unsafe fn compute(v: __m512, op: Op) -> __m512 {
        let one = _mm512_set1_ps(1.0);
        match op {
            Op::Exp => exp(v),
            Op::Silu => _mm512_div_ps(v, _mm512_add_ps(one, exp(neg(v)))),
            Op::Sigmoid => _mm512_div_ps(one, _mm512_add_ps(one, exp(neg(v)))),
        }
    }

    #[target_feature(enable = "avx512f")]
    unsafe fn neg(v: __m512) -> __m512 {
        _mm512_sub_ps(_mm512_setzero_ps(), v)
    }

    #[target_feature(enable = "avx512f")]
    unsafe fn exp(x: __m512) -> __m512 {
        let x = _mm512_min_ps(_mm512_max_ps(x, _mm512_set1_ps(LOW)), _mm512_set1_ps(HIGH));
        let n_int = _mm512_cvtps_epi32(_mm512_mul_ps(x, _mm512_set1_ps(LOG2E)));
        let n = _mm512_cvtepi32_ps(n_int);
        let r = _mm512_fnmadd_ps(n, _mm512_set1_ps(LN2_HI), x);
        let r = _mm512_fnmadd_ps(n, _mm512_set1_ps(LN2_LO), r);
        let mut p = _mm512_setzero_ps();
        for c in POLY {
            p = _mm512_fmadd_ps(p, r, _mm512_set1_ps(c));
        }
        let y = _mm512_fmadd_ps(
            p,
            _mm512_mul_ps(r, r),
            _mm512_add_ps(r, _mm512_set1_ps(1.0)),
        );
        let two_n = _mm512_slli_epi32::<23>(_mm512_add_epi32(n_int, _mm512_set1_epi32(127)));
        _mm512_mul_ps(y, _mm512_castsi512_ps(two_n))
    }
}

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use super::{Op, HIGH, LN2_HI, LN2_LO, LOG2E, LOW, POLY};
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn apply(values: &mut [f32], op: Op) {
        let (chunks, tail) = values.as_chunks_mut::<8>();
        for chunk in chunks {
            let v = _mm256_loadu_ps(chunk.as_ptr());
            _mm256_storeu_ps(chunk.as_mut_ptr(), compute(v, op));
        }
        if !tail.is_empty() {
            // The tail goes through the same lanes, so every value gets the same rounding.
            let mut padded = [0.0f32; 8];
            padded[..tail.len()].copy_from_slice(tail);
            let v = compute(_mm256_loadu_ps(padded.as_ptr()), op);
            _mm256_storeu_ps(padded.as_mut_ptr(), v);
            tail.copy_from_slice(&padded[..tail.len()]);
        }
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn compute(v: __m256, op: Op) -> __m256 {
        let one = _mm256_set1_ps(1.0);
        let neg = |v| _mm256_sub_ps(_mm256_setzero_ps(), v);
        match op {
            Op::Exp => exp(v),
            Op::Silu => _mm256_div_ps(v, _mm256_add_ps(one, exp(neg(v)))),
            Op::Sigmoid => _mm256_div_ps(one, _mm256_add_ps(one, exp(neg(v)))),
        }
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn exp(x: __m256) -> __m256 {
        let x = _mm256_min_ps(_mm256_max_ps(x, _mm256_set1_ps(LOW)), _mm256_set1_ps(HIGH));
        let n_int = _mm256_cvtps_epi32(_mm256_mul_ps(x, _mm256_set1_ps(LOG2E)));
        let n = _mm256_cvtepi32_ps(n_int);
        let r = _mm256_fnmadd_ps(n, _mm256_set1_ps(LN2_HI), x);
        let r = _mm256_fnmadd_ps(n, _mm256_set1_ps(LN2_LO), r);
        let mut p = _mm256_setzero_ps();
        for c in POLY {
            p = _mm256_fmadd_ps(p, r, _mm256_set1_ps(c));
        }
        let y = _mm256_fmadd_ps(
            p,
            _mm256_mul_ps(r, r),
            _mm256_add_ps(r, _mm256_set1_ps(1.0)),
        );
        let two_n = _mm256_slli_epi32::<23>(_mm256_add_epi32(n_int, _mm256_set1_epi32(127)));
        _mm256_mul_ps(y, _mm256_castsi256_ps(two_n))
    }
}

#[cfg(test)]
mod tests {
    use super::{exp, scalar, sigmoid, silu, Op};

    /// 1001 values across the range, a length that leaves a tail for every lane width.
    fn sweep() -> Vec<f32> {
        (0..1001)
            .map(|i| -87.0 + 175.0 * i as f32 / 1000.0)
            .collect()
    }

    fn check(name: &str, got: &[f32], inputs: &[f32], want: impl Fn(f64) -> f64, tol: f64) {
        for (g, x) in got.iter().zip(inputs) {
            let w = want(f64::from(*x));
            let error = (f64::from(*g) - w).abs() / w.abs().max(1e-30);
            assert!(
                error < tol,
                "{name}({x}) = {g}, want {w}, relative {error:.2e}"
            );
        }
    }

    #[test]
    fn exp_silu_and_sigmoid_match_double_precision() {
        let x = sweep();
        let mut e = x.clone();
        exp(&mut e);
        check("exp", &e, &x, f64::exp, 4e-7);
        // Where |silu| is tiny, e^-v is near overflow and the quotient loses digits.
        let small: Vec<f32> = x.iter().copied().filter(|v| *v > -60.0).collect();
        let mut s = small.clone();
        silu(&mut s);
        check("silu", &s, &small, |v| v / (1.0 + (-v).exp()), 1e-6);
        let mut g = small.clone();
        sigmoid(&mut g);
        check("sigmoid", &g, &small, |v| 1.0 / (1.0 + (-v).exp()), 1e-6);
    }

    #[test]
    fn the_vector_path_equals_the_scalar_polynomial() {
        let x = sweep();
        let mut fast = x.clone();
        exp(&mut fast);
        for (f, v) in fast.iter().zip(&x) {
            let s = scalar(*v, Op::Exp);
            assert!(
                (f - s).abs() <= 2.0 * f32::EPSILON * s.abs(),
                "{v}: {f} vs {s}"
            );
        }
        let mut edge = vec![-1000.0, 1000.0, 0.0, -0.0];
        exp(&mut edge);
        assert!(edge.iter().all(|v| v.is_finite() && *v >= 0.0), "{edge:?}");
    }
}
