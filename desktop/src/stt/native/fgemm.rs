//! f32 matrix product `out = x * w^T` for row-major `x` (m x k) and `w`
//! (n x k). Both operands are read along k, so every output is a dot
//! product. Register tiles keep 4 x 4 (AVX-512) or 3 x 4 (AVX2) dot
//! products in flight and reduce each one once at the end.

use super::buf::Buf;
use super::team;
use std::sync::OnceLock;

/// Rows and columns per parallel task.
const BLOCK: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Level {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx512,
}

/// The widest float path this CPU runs. `TDT_NATIVE_ISA=scalar|avx2` forces a slower one.
pub(super) fn level() -> Level {
    static LEVEL: OnceLock<Level> = OnceLock::new();
    *LEVEL.get_or_init(|| {
        let forced = std::env::var("TDT_NATIVE_ISA").unwrap_or_default();
        #[cfg(target_arch = "x86_64")]
        {
            // F16C comes with every AVX2 CPU; the f16 decoder layers need it.
            let avx2 = is_x86_feature_detected!("avx2")
                && is_x86_feature_detected!("fma")
                && is_x86_feature_detected!("f16c");
            match forced.as_str() {
                "scalar" => return Level::Scalar,
                "avx2" if avx2 => return Level::Avx2,
                _ => {}
            }
            if is_x86_feature_detected!("avx512f") {
                return Level::Avx512;
            }
            if avx2 {
                return Level::Avx2;
            }
        }
        let _ = forced;
        Level::Scalar
    })
}

#[derive(Clone, Copy)]
struct OutPtr(*mut f32);
// SAFETY: tasks write disjoint output blocks; the buffer outlives the parallel scope.
unsafe impl Send for OutPtr {}
unsafe impl Sync for OutPtr {}

/// `out[i][j] = sum_k x[i][k] * w[j][k]`, parallel over 64 x 64 blocks.
pub fn matmul(x: &[f32], m: usize, k: usize, w: &[f32], n: usize) -> Buf {
    assert!(x.len() >= m * k && w.len() >= n * k);
    let mut out = Buf::scratch(m * n);
    let (row_blocks, col_blocks) = (m.div_ceil(BLOCK), n.div_ceil(BLOCK));
    let ptr = OutPtr(out.as_mut_ptr());
    let level = level();
    team::for_each(row_blocks * col_blocks, |task| {
        let rows = (task / col_blocks) * BLOCK..((task / col_blocks + 1) * BLOCK).min(m);
        let cols = (task % col_blocks) * BLOCK..((task % col_blocks + 1) * BLOCK).min(n);
        // SAFETY: the block is inside all three buffers and belongs to this task only;
        // the level was detected from the CPU.
        unsafe { block(level, x, k, w, n, rows, cols, ptr) };
    });
    out
}

/// One block, on the calling thread.
#[allow(clippy::too_many_arguments)]
unsafe fn block(
    level: Level,
    x: &[f32],
    k: usize,
    w: &[f32],
    n: usize,
    rows: std::ops::Range<usize>,
    cols: std::ops::Range<usize>,
    out: OutPtr,
) {
    match level {
        #[cfg(target_arch = "x86_64")]
        Level::Avx512 => avx512::block(x, k, w, n, rows, cols, out.0),
        #[cfg(target_arch = "x86_64")]
        Level::Avx2 => avx2::block(x, k, w, n, rows, cols, out.0),
        Level::Scalar => {
            for i in rows {
                for j in cols.clone() {
                    *out.0.add(i * n + j) =
                        dot_scalar(&x[i * k..(i + 1) * k], &w[j * k..(j + 1) * k]);
                }
            }
        }
    }
}

fn dot_scalar(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0f32; 8];
    let (ca, ra) = a.as_chunks::<8>();
    let (cb, rb) = b.as_chunks::<8>();
    for (x, y) in ca.iter().zip(cb) {
        for ((lane, xv), yv) in lanes.iter_mut().zip(x).zip(y) {
            *lane += xv * yv;
        }
    }
    let tail: f32 = ra.iter().zip(rb).map(|(x, y)| x * y).sum();
    lanes.iter().sum::<f32>() + tail
}

#[cfg(target_arch = "x86_64")]
mod avx512 {
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx512f")]
    pub unsafe fn block(
        x: &[f32],
        k: usize,
        w: &[f32],
        n: usize,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        out: *mut f32,
    ) {
        let mut i = rows.start;
        while i < rows.end {
            let r = (rows.end - i).min(4);
            let mut j = cols.start;
            while j < cols.end {
                let c = (cols.end - j).min(4);
                let xp = x.as_ptr().add(i * k);
                let wp = w.as_ptr().add(j * k);
                let op = out.add(i * n + j);
                match (r, c) {
                    (4, 4) => tile::<4, 4>(xp, wp, k, op, n),
                    (4, 3) => tile::<4, 3>(xp, wp, k, op, n),
                    (4, 2) => tile::<4, 2>(xp, wp, k, op, n),
                    (4, 1) => tile::<4, 1>(xp, wp, k, op, n),
                    (3, 4) => tile::<3, 4>(xp, wp, k, op, n),
                    (3, 3) => tile::<3, 3>(xp, wp, k, op, n),
                    (3, 2) => tile::<3, 2>(xp, wp, k, op, n),
                    (3, 1) => tile::<3, 1>(xp, wp, k, op, n),
                    (2, 4) => tile::<2, 4>(xp, wp, k, op, n),
                    (2, 3) => tile::<2, 3>(xp, wp, k, op, n),
                    (2, 2) => tile::<2, 2>(xp, wp, k, op, n),
                    (2, 1) => tile::<2, 1>(xp, wp, k, op, n),
                    (1, 4) => tile::<1, 4>(xp, wp, k, op, n),
                    (1, 3) => tile::<1, 3>(xp, wp, k, op, n),
                    (1, 2) => tile::<1, 2>(xp, wp, k, op, n),
                    _ => tile::<1, 1>(xp, wp, k, op, n),
                }
                j += c;
            }
            i += r;
        }
    }

    /// R x C dot products of length k; the k tail uses masked loads.
    #[target_feature(enable = "avx512f")]
    #[allow(clippy::needless_range_loop)]
    unsafe fn tile<const R: usize, const C: usize>(
        x: *const f32,
        w: *const f32,
        k: usize,
        out: *mut f32,
        n: usize,
    ) {
        let mut acc = [[_mm512_setzero_ps(); C]; R];
        let mut kk = 0;
        while kk + 16 <= k {
            let mut wv = [_mm512_setzero_ps(); C];
            for (c, v) in wv.iter_mut().enumerate() {
                *v = _mm512_loadu_ps(w.add(c * k + kk));
            }
            for (r, row) in acc.iter_mut().enumerate() {
                let xv = _mm512_loadu_ps(x.add(r * k + kk));
                for (c, a) in row.iter_mut().enumerate() {
                    *a = _mm512_fmadd_ps(xv, wv[c], *a);
                }
            }
            kk += 16;
        }
        if kk < k {
            let mask: __mmask16 = (1u32 << (k - kk)) as u16 - 1;
            let mut wv = [_mm512_setzero_ps(); C];
            for (c, v) in wv.iter_mut().enumerate() {
                *v = _mm512_maskz_loadu_ps(mask, w.add(c * k + kk));
            }
            for (r, row) in acc.iter_mut().enumerate() {
                let xv = _mm512_maskz_loadu_ps(mask, x.add(r * k + kk));
                for (c, a) in row.iter_mut().enumerate() {
                    *a = _mm512_fmadd_ps(xv, wv[c], *a);
                }
            }
        }
        for (r, row) in acc.iter().enumerate() {
            for (c, a) in row.iter().enumerate() {
                *out.add(r * n + c) = _mm512_reduce_add_ps(*a);
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod avx2 {
    use std::arch::x86_64::*;

    #[target_feature(enable = "avx2,fma")]
    pub unsafe fn block(
        x: &[f32],
        k: usize,
        w: &[f32],
        n: usize,
        rows: std::ops::Range<usize>,
        cols: std::ops::Range<usize>,
        out: *mut f32,
    ) {
        let mut i = rows.start;
        while i < rows.end {
            let r = (rows.end - i).min(3);
            let mut j = cols.start;
            while j < cols.end {
                let c = (cols.end - j).min(4);
                let xp = x.as_ptr().add(i * k);
                let wp = w.as_ptr().add(j * k);
                let op = out.add(i * n + j);
                match (r, c) {
                    (3, 4) => tile::<3, 4>(xp, wp, k, op, n),
                    (3, 3) => tile::<3, 3>(xp, wp, k, op, n),
                    (3, 2) => tile::<3, 2>(xp, wp, k, op, n),
                    (3, 1) => tile::<3, 1>(xp, wp, k, op, n),
                    (2, 4) => tile::<2, 4>(xp, wp, k, op, n),
                    (2, 3) => tile::<2, 3>(xp, wp, k, op, n),
                    (2, 2) => tile::<2, 2>(xp, wp, k, op, n),
                    (2, 1) => tile::<2, 1>(xp, wp, k, op, n),
                    (1, 4) => tile::<1, 4>(xp, wp, k, op, n),
                    (1, 3) => tile::<1, 3>(xp, wp, k, op, n),
                    (1, 2) => tile::<1, 2>(xp, wp, k, op, n),
                    _ => tile::<1, 1>(xp, wp, k, op, n),
                }
                j += c;
            }
            i += r;
        }
    }

    #[target_feature(enable = "avx2,fma")]
    unsafe fn hsum(v: __m256) -> f32 {
        let pair = _mm_add_ps(_mm256_castps256_ps128(v), _mm256_extractf128_ps(v, 1));
        let quad = _mm_add_ps(pair, _mm_movehl_ps(pair, pair));
        _mm_cvtss_f32(_mm_add_ss(quad, _mm_shuffle_ps(quad, quad, 1)))
    }

    #[target_feature(enable = "avx2,fma")]
    #[allow(clippy::needless_range_loop)]
    unsafe fn tile<const R: usize, const C: usize>(
        x: *const f32,
        w: *const f32,
        k: usize,
        out: *mut f32,
        n: usize,
    ) {
        let mut acc = [[_mm256_setzero_ps(); C]; R];
        let mut kk = 0;
        while kk + 8 <= k {
            let mut wv = [_mm256_setzero_ps(); C];
            for (c, v) in wv.iter_mut().enumerate() {
                *v = _mm256_loadu_ps(w.add(c * k + kk));
            }
            for (r, row) in acc.iter_mut().enumerate() {
                let xv = _mm256_loadu_ps(x.add(r * k + kk));
                for (c, a) in row.iter_mut().enumerate() {
                    *a = _mm256_fmadd_ps(xv, wv[c], *a);
                }
            }
            kk += 8;
        }
        for r in 0..R {
            for c in 0..C {
                let mut total = hsum(acc[r][c]);
                for t in kk..k {
                    total += *x.add(r * k + t) * *w.add(c * k + t);
                }
                *out.add(r * n + c) = total;
            }
        }
    }
}
