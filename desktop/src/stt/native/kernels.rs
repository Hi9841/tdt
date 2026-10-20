//! Ternary matrix product with 8-bit activations.
//!
//! Weights stay as codes 0, 1, 2 (`w = scale * (code - 1)`). Each 128-value
//! group of an activation row is quantized to i8 with its own scale. A group
//! dot product is then `sum(code * q) - sum(q)`, done in i32 by
//! `vpdpbusd` (AVX-512 VNNI) or `vpmaddubsw` (AVX2), and scaled into f32 by
//! the weight scale and the activation scale of that group.

use super::buf::Buf;
use super::team;
use std::cell::RefCell;

pub const GROUP: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isa {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "x86_64")]
    Avx512Vnni,
}

impl Isa {
    /// Rows per packed block.
    fn rows(self) -> usize {
        match self {
            Isa::Scalar => 1,
            #[cfg(target_arch = "x86_64")]
            Isa::Avx2 => 8,
            #[cfg(target_arch = "x86_64")]
            Isa::Avx512Vnni => 16,
        }
    }

    /// The fastest path this CPU runs. `TDT_NATIVE_ISA=scalar|avx2` forces a slower one.
    pub fn detect() -> Self {
        let forced = std::env::var("TDT_NATIVE_ISA").unwrap_or_default();
        #[cfg(target_arch = "x86_64")]
        {
            let vnni = is_x86_feature_detected!("avx512f")
                && is_x86_feature_detected!("avx512bw")
                && is_x86_feature_detected!("avx512vnni");
            let avx2 = is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma");
            match forced.as_str() {
                "scalar" => return Isa::Scalar,
                "avx2" if avx2 => return Isa::Avx2,
                _ => {}
            }
            if vnni {
                return Isa::Avx512Vnni;
            }
            if avx2 {
                return Isa::Avx2;
            }
        }
        let _ = forced;
        Isa::Scalar
    }
}

/// Codes packed for one instruction set: `[n / rows][k / 4][rows][4]`.
pub struct PackedTernary {
    pub isa: Isa,
    pub n: usize,
    pub k: usize,
    codes: Vec<u8>,
    /// `[n / rows][k / GROUP][rows]`.
    scales: Vec<f32>,
}

impl PackedTernary {
    /// `codes` is row-major `n x k` (values 0..=2), `scales` is `n x (k / GROUP)`.
    pub fn new(isa: Isa, codes: &[u8], scales: &[f32], n: usize, k: usize) -> Self {
        let rows = isa.rows();
        // The SIMD tiles need whole blocks; other shapes run the scalar path.
        let isa = if k.is_multiple_of(GROUP) && n.is_multiple_of(2 * rows) {
            isa
        } else {
            Isa::Scalar
        };
        let rows = isa.rows();
        let groups = k / GROUP;
        let mut packed = vec![0u8; n * k];
        let mut packed_scales = vec![0.0f32; n * groups];
        for block in 0..n / rows {
            for k4 in 0..k / 4 {
                for r in 0..rows {
                    let row = block * rows + r;
                    let dst = ((block * (k / 4) + k4) * rows + r) * 4;
                    packed[dst..dst + 4].copy_from_slice(&codes[row * k + k4 * 4..][..4]);
                }
            }
            for g in 0..groups {
                for r in 0..rows {
                    packed_scales[(block * groups + g) * rows + r] =
                        scales[(block * rows + r) * groups + g];
                }
            }
        }
        Self {
            isa,
            n,
            k,
            codes: packed,
            scales: packed_scales,
        }
    }

    /// `x` is `m x k` f32; returns `m x n`.
    pub fn forward(&self, x: &[f32], m: usize) -> Buf {
        let mut out = Buf::zeroed(m * self.n);
        self.forward_into(x, m, &mut out);
        out
    }

    /// Writes `m x n` into `out`, which must hold exactly that many values.
    pub fn forward_into(&self, x: &[f32], m: usize, out: &mut [f32]) {
        assert_eq!(out.len(), m * self.n);
        let q = Quantized::new(self.isa, x, m, self.k);
        let ptr = OutPtr(out.as_mut_ptr());
        let rows = self.isa.rows();
        // Two blocks per task: the SIMD tiles cover 2 * rows outputs.
        let tasks = self.n / (2 * rows).max(1);
        let tasks = if self.isa == Isa::Scalar {
            self.n
        } else {
            tasks
        };
        team::for_each(tasks, |task| match self.isa {
            Isa::Scalar => self.scalar_rows(&q, task, ptr),
            #[cfg(target_arch = "x86_64")]
            // SAFETY: Isa::detect confirmed the CPU features.
            Isa::Avx2 => unsafe { self.avx2_task(&q, task, ptr) },
            #[cfg(target_arch = "x86_64")]
            // SAFETY: Isa::detect confirmed the CPU features.
            Isa::Avx512Vnni => unsafe { self.vnni_task(&q, task, ptr) },
        });
    }

    fn scalar_rows(&self, q: &Quantized, row: usize, out: OutPtr) {
        let (k, groups) = (self.k, self.k / GROUP);
        let rows = self.isa.rows();
        let (block, r) = (row / rows, row % rows);
        for frame in 0..q.m {
            let mut total = 0.0f32;
            for g in 0..groups {
                let mut sum = 0i32;
                for kk in g * GROUP..(g + 1) * GROUP {
                    let code = self.codes[((block * (k / 4) + kk / 4) * rows + r) * 4 + kk % 4];
                    sum += i32::from(code) * i32::from(q.values[frame * k + kk]);
                }
                let dot = sum - q.sums[frame * groups + g];
                total += dot as f32
                    * self.scales[(block * groups + g) * rows + r]
                    * q.scales[frame * groups + g];
            }
            // SAFETY: each output element belongs to one task.
            unsafe { *out.0.add(frame * self.n + row) = total };
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx512f,avx512bw,avx512vnni")]
    unsafe fn vnni_task(&self, q: &Quantized, task: usize, out: OutPtr) {
        let mut frame = 0;
        while frame + 6 <= q.m {
            self.vnni_tile::<6>(q, task, frame, out);
            frame += 6;
        }
        match q.m - frame {
            1 => self.vnni_tile::<1>(q, task, frame, out),
            2 => self.vnni_tile::<2>(q, task, frame, out),
            3 => self.vnni_tile::<3>(q, task, frame, out),
            4 => self.vnni_tile::<4>(q, task, frame, out),
            5 => self.vnni_tile::<5>(q, task, frame, out),
            _ => {}
        }
    }

    /// 32 outputs (two 16-row blocks) by F frames.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx512f,avx512bw,avx512vnni")]
    #[allow(clippy::needless_range_loop)]
    unsafe fn vnni_tile<const F: usize>(
        &self,
        q: &Quantized,
        task: usize,
        frame0: usize,
        out: OutPtr,
    ) {
        use std::arch::x86_64::*;
        let (k, k4s, groups) = (self.k, self.k / 4, self.k / GROUP);
        let blocks = [2 * task, 2 * task + 1];
        let mut acc = [[_mm512_setzero_ps(); F]; 2];
        for g in 0..groups {
            let mut dots = [[_mm512_setzero_si512(); F]; 2];
            for step in 0..GROUP / 4 {
                let k4 = g * (GROUP / 4) + step;
                let w0 =
                    _mm512_loadu_si512(self.codes.as_ptr().add((blocks[0] * k4s + k4) * 64).cast());
                let w1 =
                    _mm512_loadu_si512(self.codes.as_ptr().add((blocks[1] * k4s + k4) * 64).cast());
                for f in 0..F {
                    let src = q.values.as_ptr().add((frame0 + f) * k + k4 * 4);
                    let x = _mm512_set1_epi32(std::ptr::read_unaligned(src.cast::<i32>()));
                    dots[0][f] = _mm512_dpbusd_epi32(dots[0][f], w0, x);
                    dots[1][f] = _mm512_dpbusd_epi32(dots[1][f], w1, x);
                }
            }
            for (b, block) in blocks.iter().enumerate() {
                let wscale = _mm512_loadu_ps(self.scales.as_ptr().add((block * groups + g) * 16));
                for f in 0..F {
                    let index = (frame0 + f) * groups + g;
                    let corrected = _mm512_sub_epi32(dots[b][f], _mm512_set1_epi32(q.sums[index]));
                    let scale = _mm512_mul_ps(wscale, _mm512_set1_ps(q.scales[index]));
                    acc[b][f] = _mm512_fmadd_ps(_mm512_cvtepi32_ps(corrected), scale, acc[b][f]);
                }
            }
        }
        for (b, block) in blocks.iter().enumerate() {
            for f in 0..F {
                _mm512_storeu_ps(out.0.add((frame0 + f) * self.n + block * 16), acc[b][f]);
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,fma")]
    unsafe fn avx2_task(&self, q: &Quantized, task: usize, out: OutPtr) {
        for block in [2 * task, 2 * task + 1] {
            let mut frame = 0;
            while frame + 4 <= q.m {
                self.avx2_tile::<4>(q, block, frame, out);
                frame += 4;
            }
            match q.m - frame {
                1 => self.avx2_tile::<1>(q, block, frame, out),
                2 => self.avx2_tile::<2>(q, block, frame, out),
                3 => self.avx2_tile::<3>(q, block, frame, out),
                _ => {}
            }
        }
    }

    /// 8 outputs by F frames.
    #[cfg(target_arch = "x86_64")]
    #[target_feature(enable = "avx2,fma")]
    #[allow(clippy::needless_range_loop)]
    unsafe fn avx2_tile<const F: usize>(
        &self,
        q: &Quantized,
        block: usize,
        frame0: usize,
        out: OutPtr,
    ) {
        use std::arch::x86_64::*;
        let (k, k4s, groups) = (self.k, self.k / 4, self.k / GROUP);
        let ones = _mm256_set1_epi16(1);
        let mut acc = [_mm256_setzero_ps(); F];
        for g in 0..groups {
            let mut dots = [_mm256_setzero_si256(); F];
            for step in 0..GROUP / 4 {
                let k4 = g * (GROUP / 4) + step;
                let w = _mm256_loadu_si256(self.codes.as_ptr().add((block * k4s + k4) * 32).cast());
                for f in 0..F {
                    let src = q.values.as_ptr().add((frame0 + f) * k + k4 * 4);
                    let x = _mm256_set1_epi32(std::ptr::read_unaligned(src.cast::<i32>()));
                    // Codes are at most 2 and |q| at most 127: pair sums fit in i16.
                    let pairs = _mm256_maddubs_epi16(w, x);
                    dots[f] = _mm256_add_epi32(dots[f], _mm256_madd_epi16(pairs, ones));
                }
            }
            let wscale = _mm256_loadu_ps(self.scales.as_ptr().add((block * groups + g) * 8));
            for f in 0..F {
                let index = (frame0 + f) * groups + g;
                let corrected = _mm256_sub_epi32(dots[f], _mm256_set1_epi32(q.sums[index]));
                let scale = _mm256_mul_ps(wscale, _mm256_set1_ps(q.scales[index]));
                acc[f] = _mm256_fmadd_ps(_mm256_cvtepi32_ps(corrected), scale, acc[f]);
            }
        }
        for f in 0..F {
            _mm256_storeu_ps(out.0.add((frame0 + f) * self.n + block * 8), acc[f]);
        }
    }
}

/// Activations as i8 with one scale and one sum per 128-value group.
pub struct Quantized {
    m: usize,
    values: Vec<i8>,
    scales: Vec<f32>,
    sums: Vec<i32>,
}

impl Quantized {
    pub fn new(isa: Isa, x: &[f32], m: usize, k: usize) -> Self {
        let groups = k / GROUP;
        // Reuse this thread's buffer: a fresh one would fault in new pages.
        let mut values = QUANTIZED.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
        values.clear();
        values.resize(m * k, 0);
        let mut scales = vec![0.0f32; m * groups];
        let mut sums = vec![0i32; m * groups];
        let (all_values, all_scales, all_sums) = (
            team::SendPtr::new(&mut values),
            team::SendPtr::new(&mut scales),
            team::SendPtr::new(&mut sums),
        );
        team::for_each(m.div_ceil(QUANTIZE_ROWS), |block| {
            for row in block * QUANTIZE_ROWS..((block + 1) * QUANTIZE_ROWS).min(m) {
                // SAFETY: each row belongs to one task; the buffers outlive the step.
                let (values, scales, sums) = unsafe {
                    (
                        all_values.slice_mut(row * k, k),
                        all_scales.slice_mut(row * groups, groups),
                        all_sums.slice_mut(row * groups, groups),
                    )
                };
                let src = &x[row * k..(row + 1) * k];
                for g in 0..groups {
                    let span = g * GROUP..(g + 1) * GROUP;
                    let (scale, sum) = match isa {
                        #[cfg(target_arch = "x86_64")]
                        // SAFETY: Isa::detect confirmed the CPU features.
                        Isa::Avx512Vnni => unsafe {
                            quantize_avx512(&src[span.clone()], &mut values[span])
                        },
                        #[cfg(target_arch = "x86_64")]
                        // SAFETY: Isa::detect confirmed the CPU features.
                        Isa::Avx2 => unsafe {
                            quantize_avx2(&src[span.clone()], &mut values[span])
                        },
                        Isa::Scalar => quantize_scalar(&src[span.clone()], &mut values[span]),
                    };
                    scales[g] = scale;
                    sums[g] = sum;
                }
            }
        });
        Self {
            m,
            values,
            scales,
            sums,
        }
    }
}

/// Rows per quantization task: a short take still splits, a row is never shared.
const QUANTIZE_ROWS: usize = 8;

thread_local! {
    static QUANTIZED: RefCell<Vec<i8>> = const { RefCell::new(Vec::new()) };
}

impl Drop for Quantized {
    fn drop(&mut self) {
        let values = std::mem::take(&mut self.values);
        QUANTIZED.with(|cell| *cell.borrow_mut() = values);
    }
}

fn group_scale(max: f32) -> f32 {
    if max > 0.0 {
        max / 127.0
    } else {
        1.0
    }
}

/// One 128-value group to i8, rounding half to even like `cvtps2dq`.
fn quantize_scalar(src: &[f32], dst: &mut [i8]) -> (f32, i32) {
    let scale = group_scale(src.iter().fold(0.0f32, |m, v| m.max(v.abs())));
    let inv = 1.0 / scale;
    let mut sum = 0i32;
    for (d, v) in dst.iter_mut().zip(src) {
        let code = (v * inv).round_ties_even().clamp(-127.0, 127.0) as i8;
        *d = code;
        sum += i32::from(code);
    }
    (scale, sum)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512bw")]
unsafe fn quantize_avx512(src: &[f32], dst: &mut [i8]) -> (f32, i32) {
    use std::arch::x86_64::*;
    let p = src.as_ptr();
    let mut vmax = _mm512_setzero_ps();
    for i in 0..GROUP / 16 {
        vmax = _mm512_max_ps(vmax, _mm512_abs_ps(_mm512_loadu_ps(p.add(i * 16))));
    }
    let scale = group_scale(_mm512_reduce_max_ps(vmax));
    let inv = _mm512_set1_ps(1.0 / scale);
    let limit = _mm512_set1_epi32(127);
    let mut vsum = _mm512_setzero_si512();
    for i in 0..GROUP / 16 {
        let q = _mm512_cvtps_epi32(_mm512_mul_ps(_mm512_loadu_ps(p.add(i * 16)), inv));
        let q = _mm512_max_epi32(
            _mm512_min_epi32(q, limit),
            _mm512_sub_epi32(_mm512_setzero_si512(), limit),
        );
        vsum = _mm512_add_epi32(vsum, q);
        _mm_storeu_si128(
            dst.as_mut_ptr().add(i * 16).cast(),
            _mm512_cvtsepi32_epi8(q),
        );
    }
    (scale, _mm512_reduce_add_epi32(vsum))
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn quantize_avx2(src: &[f32], dst: &mut [i8]) -> (f32, i32) {
    use std::arch::x86_64::*;
    let p = src.as_ptr();
    let sign = _mm256_set1_ps(-0.0);
    let mut vmax = _mm256_setzero_ps();
    for i in 0..GROUP / 8 {
        vmax = _mm256_max_ps(vmax, _mm256_andnot_ps(sign, _mm256_loadu_ps(p.add(i * 8))));
    }
    let mut lanes = [0.0f32; 8];
    _mm256_storeu_ps(lanes.as_mut_ptr(), vmax);
    let scale = group_scale(lanes.iter().fold(0.0f32, |m, v| m.max(*v)));
    let inv = _mm256_set1_ps(1.0 / scale);
    let limit = _mm256_set1_epi32(127);
    let low = _mm256_set1_epi32(-127);
    let order = _mm256_setr_epi32(0, 4, 1, 5, 2, 6, 3, 7);
    let mut vsum = _mm256_setzero_si256();
    for i in 0..GROUP / 32 {
        let mut q = [_mm256_setzero_si256(); 4];
        for (j, lane) in q.iter_mut().enumerate() {
            let v = _mm256_mul_ps(_mm256_loadu_ps(p.add(i * 32 + j * 8)), inv);
            *lane = _mm256_max_epi32(_mm256_min_epi32(_mm256_cvtps_epi32(v), limit), low);
            vsum = _mm256_add_epi32(vsum, *lane);
        }
        let words = _mm256_packs_epi16(
            _mm256_packs_epi32(q[0], q[1]),
            _mm256_packs_epi32(q[2], q[3]),
        );
        _mm256_storeu_si256(
            dst.as_mut_ptr().add(i * 32).cast(),
            _mm256_permutevar8x32_epi32(words, order),
        );
    }
    let mut sums = [0i32; 8];
    _mm256_storeu_si256(sums.as_mut_ptr().cast(), vsum);
    (scale, sums.iter().sum())
}

#[derive(Clone, Copy)]
struct OutPtr(*mut f32);
// SAFETY: tasks write disjoint output elements; the buffer outlives the parallel scope.
unsafe impl Send for OutPtr {}
unsafe impl Sync for OutPtr {}

#[cfg(test)]
mod tests {
    use super::{Isa, PackedTernary, GROUP};

    fn case(n: usize, k: usize, m: usize) -> (Vec<u8>, Vec<f32>, Vec<f32>) {
        let codes = (0..n * k).map(|i| ((i * 7 + i / 13) % 3) as u8).collect();
        let scales = (0..n * k / GROUP)
            .map(|i| 0.01 + (i % 5) as f32 * 0.003)
            .collect();
        let x = (0..m * k)
            .map(|i| ((i * 31 % 97) as f32 - 48.0) * 0.02)
            .collect();
        (codes, scales, x)
    }

    fn exact(codes: &[u8], scales: &[f32], x: &[f32], n: usize, k: usize, m: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; m * n];
        for f in 0..m {
            for r in 0..n {
                out[f * n + r] = (0..k)
                    .map(|c| {
                        x[f * k + c]
                            * (f32::from(codes[r * k + c]) - 1.0)
                            * scales[r * (k / GROUP) + c / GROUP]
                    })
                    .sum();
            }
        }
        out
    }

    #[test]
    fn every_path_matches_the_float_product_within_quantization_error() {
        let (n, k, m) = (64, 256, 7);
        let (codes, scales, x) = case(n, k, m);
        let reference = exact(&codes, &scales, &x, n, k, m);
        let scalar = PackedTernary::new(Isa::Scalar, &codes, &scales, n, k).forward(&x, m);
        let norm: f32 = reference.iter().map(|v| v * v).sum::<f32>().sqrt();
        let error: f32 = scalar
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            .sqrt();
        assert!(error / norm < 0.01, "quantization error {}", error / norm);
        let fast = PackedTernary::new(Isa::detect(), &codes, &scales, n, k).forward(&x, m);
        for (a, b) in fast.iter().zip(scalar.iter()) {
            assert!((a - b).abs() <= 1e-4 * b.abs().max(1.0), "{a} vs {b}");
        }
    }

    #[test]
    fn simd_quantization_equals_the_scalar_rounding() {
        let x: Vec<f32> = (0..3 * 256)
            .map(|i| ((i * 37 % 211) as f32 - 105.0) * 0.013)
            .collect();
        let reference = super::Quantized::new(Isa::Scalar, &x, 3, 256);
        #[cfg(target_arch = "x86_64")]
        for isa in [Isa::Avx2, Isa::Avx512Vnni] {
            let supported = match isa {
                Isa::Avx2 => is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma"),
                _ => Isa::detect() == Isa::Avx512Vnni,
            };
            if !supported {
                continue;
            }
            let fast = super::Quantized::new(isa, &x, 3, 256);
            assert_eq!(fast.values, reference.values, "{isa:?}");
            assert_eq!(fast.sums, reference.sums, "{isa:?}");
            assert_eq!(fast.scales, reference.scales, "{isa:?}");
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn the_avx2_path_matches_the_scalar_path() {
        if !is_x86_feature_detected!("avx2") || !is_x86_feature_detected!("fma") {
            return;
        }
        let (n, k, m) = (32, 128, 5);
        let (codes, scales, x) = case(n, k, m);
        let scalar = PackedTernary::new(Isa::Scalar, &codes, &scales, n, k).forward(&x, m);
        let avx2 = PackedTernary::new(Isa::Avx2, &codes, &scales, n, k).forward(&x, m);
        for (a, b) in avx2.iter().zip(scalar.iter()) {
            assert!((a - b).abs() <= 1e-4 * b.abs().max(1.0), "{a} vs {b}");
        }
    }
}

#[cfg(test)]
mod bench {
    use super::{Isa, PackedTernary};

    #[test]
    #[ignore = "timing only"]
    fn kernel_throughput() {
        for (m, n, k) in [
            (29usize, 1024usize, 1024usize),
            (29, 4096, 1024),
            (29, 1024, 4096),
            (77, 4096, 1024),
        ] {
            let codes: Vec<u8> = (0..n * k).map(|i| (i % 3) as u8).collect();
            let scales = vec![0.01f32; n * k / 128];
            let x: Vec<f32> = (0..m * k).map(|i| (i % 17) as f32 * 0.1 - 0.8).collect();
            let layer = PackedTernary::new(Isa::detect(), &codes, &scales, n, k);
            let _ = layer.forward(&x, m);
            let runs = 20;
            let started = std::time::Instant::now();
            for _ in 0..runs {
                std::hint::black_box(layer.forward(&x, m));
            }
            let secs = started.elapsed().as_secs_f64() / runs as f64;
            let quant = {
                let started = std::time::Instant::now();
                for _ in 0..runs {
                    std::hint::black_box(super::Quantized::new(layer.isa, &x, m, k));
                }
                started.elapsed().as_secs_f64() / runs as f64
            };
            let macs = (m * n * k) as f64;
            println!(
                "{:?} m{m} n{n} k{k}: {:.3} ms ({:.0} GMAC/s), quantize {:.3} ms",
                layer.isa,
                secs * 1e3,
                macs / secs / 1e9,
                quant * 1e3
            );
        }
    }
}
