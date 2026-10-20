//! Parakeet TDT: a FastConformer encoder and a token-and-duration transducer.
//! The structure follows the Apache-2.0 Hugging Face `ParakeetForTDT`.

use super::buf::Buf;
use super::fgemm;
use super::gemm::{Dense, Precision, Ternary};
use super::half::HalfDense;
use super::team;
use super::tensors::Tensors;
use parking_lot::RwLock;
use std::sync::Arc;

const LAYER_NORM_EPS: f32 = 1e-5;
const BATCH_NORM_EPS: f32 = 1e-5;

#[derive(Debug, Clone)]
pub struct Config {
    pub hidden: usize,
    pub ff: usize,
    pub heads: usize,
    pub layers: usize,
    pub mels: usize,
    pub conv_kernel: usize,
    pub sub_channels: usize,
    pub decoder_hidden: usize,
    pub decoder_layers: usize,
    pub vocab: usize,
    pub blank: usize,
    pub durations: Vec<usize>,
    pub max_symbols: usize,
    pub group: usize,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let json: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("config.json is not JSON: {e}"))?;
        let enc = &json["encoder_config"];
        let num = |value: &serde_json::Value, key: &str| -> Result<usize, String> {
            value[key]
                .as_u64()
                .map(|v| v as usize)
                .ok_or_else(|| format!("config.json has no {key}"))
        };
        // Only the shape this engine implements is accepted.
        if num(enc, "subsampling_factor")? != 8
            || num(enc, "subsampling_conv_kernel_size")? != 3
            || num(enc, "subsampling_conv_stride")? != 2
            || enc["scale_input"].as_bool() == Some(true)
            || json["model_type"].as_str() != Some("parakeet_tdt")
        {
            return Err("config.json describes a model shape this engine does not run".into());
        }
        let config = Self {
            hidden: num(enc, "hidden_size")?,
            ff: num(enc, "intermediate_size")?,
            heads: num(enc, "num_attention_heads")?,
            layers: num(enc, "num_hidden_layers")?,
            mels: num(enc, "num_mel_bins")?,
            conv_kernel: num(enc, "conv_kernel_size")?,
            sub_channels: num(enc, "subsampling_conv_channels")?,
            decoder_hidden: num(&json, "decoder_hidden_size")?,
            decoder_layers: num(&json, "num_decoder_layers")?,
            vocab: num(&json, "vocab_size")?,
            blank: num(&json, "blank_token_id")?,
            durations: json["durations"]
                .as_array()
                .ok_or("config.json has no durations")?
                .iter()
                .map(|d| d.as_u64().map(|d| d as usize).ok_or("bad duration"))
                .collect::<Result<_, _>>()?,
            max_symbols: num(&json, "max_symbols_per_step")?,
            group: num(&json, "ternary_group_size")?,
        };
        if !config.hidden.is_multiple_of(config.heads)
            || !config.mels.is_multiple_of(8)
            || config.durations.is_empty()
        {
            return Err("config.json has inconsistent sizes".into());
        }
        Ok(config)
    }
}

struct Norm {
    weight: Vec<f32>,
    bias: Vec<f32>,
}

impl Norm {
    fn load(t: &Tensors, name: &str, size: usize) -> Result<Self, String> {
        Ok(Self {
            weight: t.f32_len(&format!("{name}.weight"), size)?,
            bias: t.f32_len(&format!("{name}.bias"), size)?,
        })
    }

    fn apply(&self, x: &[f32]) -> Buf {
        let size = self.weight.len();
        let mut out = Buf::zeroed(x.len());
        team::chunks_mut(&mut out, size, |index, o| {
            let row = &x[index * size..(index + 1) * size];
            let mean = row.iter().sum::<f32>() / size as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / size as f32;
            let inv = 1.0 / (var + LAYER_NORM_EPS).sqrt();
            for i in 0..size {
                o[i] = (row[i] - mean) * inv * self.weight[i] + self.bias[i];
            }
        });
        out
    }

    /// `x += scale * y`, then the normalized `x`, in one pass per row.
    fn residual(&self, x: &mut [f32], y: &[f32], scale: f32) -> Buf {
        let size = self.weight.len();
        let mut out = Buf::zeroed(x.len());
        let rows = team::SendPtr::new(x);
        team::chunks_mut(&mut out, size, |index, o| {
            // SAFETY: each row of `x` belongs to the task of the same output row.
            let row = unsafe { rows.slice_mut(index * size, size) };
            let add = &y[index * size..(index + 1) * size];
            for (v, a) in row.iter_mut().zip(add) {
                *v += scale * a;
            }
            let mean = row.iter().sum::<f32>() / size as f32;
            let var = row.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / size as f32;
            let inv = 1.0 / (var + LAYER_NORM_EPS).sqrt();
            for i in 0..size {
                o[i] = (row[i] - mean) * inv * self.weight[i] + self.bias[i];
            }
        });
        out
    }
}

fn ternary(
    t: &Tensors,
    name: &str,
    out: usize,
    inp: usize,
    group: usize,
    precision: Precision,
) -> Result<Ternary, String> {
    let scales = t.f32_len(&format!("{name}.scales"), out * (inp / group))?;
    Ternary::from_packed(
        t.u8(&format!("{name}.qweight"))?,
        scales,
        out,
        inp,
        group,
        precision,
    )
}

/// Several attention layers with the same input, stacked by output rows.
fn stacked(
    t: &Tensors,
    prefix: &str,
    names: &[&str],
    (out, inp): (usize, usize),
    group: usize,
    precision: Precision,
) -> Result<Ternary, String> {
    let mut packed = Vec::new();
    let mut scales = Vec::new();
    for name in names {
        let name = format!("{prefix}.self_attn.{name}");
        packed.extend_from_slice(t.u8(&format!("{name}.qweight"))?);
        scales.extend(t.f32_len(&format!("{name}.scales"), out * (inp / group))?);
    }
    Ternary::from_packed(&packed, scales, names.len() * out, inp, group, precision)
}

/// Decoder layer with f16 weights as stored and an f32 bias.
fn half_dense(t: &Tensors, name: &str, out: usize, inp: usize) -> Result<HalfDense, String> {
    let weight = t.f16_bits(&format!("{name}.weight"), out * inp)?;
    let bias = t.f32_len(&format!("{name}.bias"), out)?;
    HalfDense::new(weight, Some(bias), out, inp)
}

fn dense(t: &Tensors, name: &str, out: usize, inp: usize, bias: bool) -> Result<Dense, String> {
    let weight = t.f32_len(&format!("{name}.weight"), out * inp)?;
    let bias = if bias {
        Some(t.f32_len(&format!("{name}.bias"), out)?)
    } else {
        None
    };
    Dense::new(weight, bias, out, inp)
}

fn silu(v: f32) -> f32 {
    v / (1.0 + (-v).exp())
}

fn sigmoid(v: f32) -> f32 {
    1.0 / (1.0 + (-v).exp())
}

/// Stride-2 convolution stack that turns 10 ms mel frames into 80 ms frames.
struct Subsampling {
    channels: usize,
    /// First conv as `[tap][channel]` weights and bias.
    conv0: (Vec<f32>, Vec<f32>),
    dw1: (Vec<f32>, Vec<f32>),
    pw1: Dense,
    dw2: (Vec<f32>, Vec<f32>),
    pw2: Dense,
    linear: Dense,
}

impl Subsampling {
    fn load(t: &Tensors, c: &Config) -> Result<Self, String> {
        let ch = c.sub_channels;
        let conv = |index: usize| -> Result<(Vec<f32>, Vec<f32>), String> {
            Ok((
                t.f32_len(
                    &format!("encoder.subsampling.layers.{index}.weight"),
                    ch * 9,
                )?,
                t.f32_len(&format!("encoder.subsampling.layers.{index}.bias"), ch)?,
            ))
        };
        Ok(Self {
            channels: ch,
            conv0: {
                let (weight, bias) = conv(0)?;
                (
                    (0..9 * ch).map(|i| weight[(i % ch) * 9 + i / ch]).collect(),
                    bias,
                )
            },
            dw1: conv(2)?,
            pw1: dense(t, "encoder.subsampling.layers.3", ch, ch, true)?,
            dw2: conv(5)?,
            pw2: dense(t, "encoder.subsampling.layers.6", ch, ch, true)?,
            linear: dense(
                t,
                "encoder.subsampling.linear",
                c.hidden,
                ch * (c.mels / 8),
                true,
            )?,
        })
    }

    /// `features` is `frames x mels`; returns `(out_frames x hidden, out_frames)`.
    fn forward(&self, features: &[f32], frames: usize, mels: usize) -> (Buf, usize) {
        let ch = self.channels;
        // First conv: one input channel, output channels-last [h][w][c].
        let (h1, w1) = (half(frames), half(mels));
        let mut x = Buf::zeroed(h1 * w1 * ch);
        let (weight, bias) = &self.conv0;
        team::chunks_mut(&mut x, w1 * ch, |h, row| {
            for w in 0..w1 {
                let cell = &mut row[w * ch..(w + 1) * ch];
                cell.copy_from_slice(bias);
                for tap in 0..9 {
                    let Some(v) = sample(features, frames, mels, 2 * h + tap / 3, 2 * w + tap % 3)
                    else {
                        continue;
                    };
                    for (o, wt) in cell.iter_mut().zip(&weight[tap * ch..(tap + 1) * ch]) {
                        *o += wt * v;
                    }
                }
                cell.iter_mut().for_each(|o| *o = o.max(0.0));
            }
        });
        let (x, h2, w2) = depthwise(&x, h1, w1, ch, &self.dw1);
        let mut x = self.pw1.forward(&x, h2 * w2);
        x.iter_mut().for_each(|v| *v = v.max(0.0));
        let (x, h3, w3) = depthwise(&x, h2, w2, ch, &self.dw2);
        let mut x = self.pw2.forward(&x, h3 * w3);
        x.iter_mut().for_each(|v| *v = v.max(0.0));
        // Flatten per frame as channel-major: index = c * w3 + w.
        let mut flat = Buf::zeroed(h3 * ch * w3);
        for h in 0..h3 {
            for w in 0..w3 {
                for c in 0..ch {
                    flat[h * ch * w3 + c * w3 + w] = x[(h * w3 + w) * ch + c];
                }
            }
        }
        (self.linear.forward(&flat, h3), h3)
    }
}

/// Output length of a kernel-3, stride-2, padding-1 convolution.
fn half(n: usize) -> usize {
    (n - 1) / 2 + 1
}

/// Input value at padded position (row, col), or None in the zero padding.
fn sample(x: &[f32], rows: usize, cols: usize, row: usize, col: usize) -> Option<f32> {
    let (r, c) = (row.checked_sub(1)?, col.checked_sub(1)?);
    (r < rows && c < cols).then(|| x[r * cols + c])
}

/// Depthwise 3x3 stride-2 convolution on channels-last data.
fn depthwise(
    x: &[f32],
    h: usize,
    w: usize,
    ch: usize,
    (weight, bias): &(Vec<f32>, Vec<f32>),
) -> (Buf, usize, usize) {
    let (ho, wo) = (half(h), half(w));
    let mut out = Buf::zeroed(ho * wo * ch);
    team::chunks_mut(&mut out, wo * ch, |oh, row| {
        for ow in 0..wo {
            let cell = &mut row[ow * ch..(ow + 1) * ch];
            cell.copy_from_slice(bias);
            for i in 0..3 {
                for j in 0..3 {
                    let (ih, iw) = (2 * oh + i, 2 * ow + j);
                    if ih == 0 || iw == 0 || ih > h || iw > w {
                        continue;
                    }
                    let input = &x[((ih - 1) * w + (iw - 1)) * ch..][..ch];
                    for c in 0..ch {
                        cell[c] += weight[c * 9 + i * 3 + j] * input[c];
                    }
                }
            }
        }
    });
    (out, ho, wo)
}

struct Block {
    norm_ff1: Norm,
    ff1: (Ternary, Ternary),
    norm_att: Norm,
    /// q, k, and v stacked into one `3d x d` layer: one quantization, one product.
    qkv: Ternary,
    o: Ternary,
    rel_k: Ternary,
    /// `rel_k` applied to the position table, kept between takes.
    rel_cache: RelCache,
    bias_u: Vec<f32>,
    bias_v: Vec<f32>,
    norm_conv: Norm,
    pw1: Ternary,
    /// Depthwise conv weights as `[tap][channel]`, so a tap is one contiguous row.
    dw_by_tap: Vec<f32>,
    bn_scale: Vec<f32>,
    bn_shift: Vec<f32>,
    pw2: Ternary,
    norm_ff2: Norm,
    ff2: (Ternary, Ternary),
    norm_out: Norm,
}

impl Block {
    fn load(t: &Tensors, c: &Config, index: usize, precision: Precision) -> Result<Self, String> {
        let p = format!("encoder.layers.{index}");
        let (d, g) = (c.hidden, c.group);
        let tern = |name: &str, out: usize, inp: usize| {
            ternary(t, &format!("{p}.{name}"), out, inp, g, precision)
        };
        let mean = t.f32_len(&format!("{p}.conv.norm.running_mean"), d)?;
        let var = t.f32_len(&format!("{p}.conv.norm.running_var"), d)?;
        let weight = t.f32_len(&format!("{p}.conv.norm.weight"), d)?;
        let bias = t.f32_len(&format!("{p}.conv.norm.bias"), d)?;
        let bn_scale: Vec<f32> = (0..d)
            .map(|i| weight[i] / (var[i] + BATCH_NORM_EPS).sqrt())
            .collect();
        let bn_shift = (0..d).map(|i| bias[i] - mean[i] * bn_scale[i]).collect();
        Ok(Self {
            norm_ff1: Norm::load(t, &format!("{p}.norm_feed_forward1"), d)?,
            ff1: (
                tern("feed_forward1.linear1", c.ff, d)?,
                tern("feed_forward1.linear2", d, c.ff)?,
            ),
            norm_att: Norm::load(t, &format!("{p}.norm_self_att"), d)?,
            qkv: stacked(t, &p, &["q_proj", "k_proj", "v_proj"], (d, d), g, precision)?,
            o: tern("self_attn.o_proj", d, d)?,
            rel_k: tern("self_attn.relative_k_proj", d, d)?,
            rel_cache: RelCache::default(),
            bias_u: t.f32_len(&format!("{p}.self_attn.bias_u"), d)?,
            bias_v: t.f32_len(&format!("{p}.self_attn.bias_v"), d)?,
            norm_conv: Norm::load(t, &format!("{p}.norm_conv"), d)?,
            pw1: tern("conv.pointwise_conv1", 2 * d, d)?,
            dw_by_tap: {
                let kernel = c.conv_kernel;
                let dw = t.f32_len(&format!("{p}.conv.depthwise_conv.weight"), d * kernel)?;
                (0..kernel * d)
                    .map(|i| dw[(i % d) * kernel + i / d])
                    .collect()
            },
            bn_scale,
            bn_shift,
            pw2: tern("conv.pointwise_conv2", d, d)?,
            norm_ff2: Norm::load(t, &format!("{p}.norm_feed_forward2"), d)?,
            ff2: (
                tern("feed_forward2.linear1", c.ff, d)?,
                tern("feed_forward2.linear2", d, c.ff)?,
            ),
            norm_out: Norm::load(t, &format!("{p}.norm_out"), d)?,
        })
    }

    fn feed_forward((up, down): &(Ternary, Ternary), x: &[f32], frames: usize) -> Buf {
        let mut hidden = up.forward(x, frames);
        team::chunks_mut(&mut hidden, 4096, |_, values| {
            values.iter_mut().for_each(|v| *v = silu(*v));
        });
        down.forward(&hidden, frames)
    }

    fn forward(&self, x: &mut Buf, frames: usize, c: &Config) {
        let ff = Self::feed_forward(&self.ff1, &self.norm_ff1.apply(x), frames);
        let normed = self.norm_att.residual(x, &ff, 0.5);
        let att = self.attention(&normed, frames, c);
        let normed = self.norm_conv.residual(x, &att, 1.0);
        let conv = self.convolution(&normed, frames, c);
        let normed = self.norm_ff2.residual(x, &conv, 1.0);
        let ff = Self::feed_forward(&self.ff2, &normed, frames);
        *x = self.norm_out.residual(x, &ff, 0.5);
    }

    /// Multi-head attention with Transformer-XL relative positions.
    fn attention(&self, x: &[f32], t: usize, c: &Config) -> Buf {
        let (d, heads) = (c.hidden, c.heads);
        let hd = d / heads;
        let qkv = self.qkv.forward(x, t);
        let cache = self
            .rel_cache
            .rows(t, d, |table, rows| self.rel_k.forward(table, rows));
        let r = cache.slice(t, d);
        let scale = 1.0 / (hd as f32).sqrt();
        let p = 2 * t - 1;
        let mut context = Buf::zeroed(t * d);
        let per_head: Vec<Buf> = team::map(heads, |h| {
            let cols = h * hd..(h + 1) * hd;
            // Columns of head h from a matrix with `stride` values per row.
            let head =
                |src: &[f32], stride: usize, offset: usize, rows: usize, bias: Option<&[f32]>| {
                    let mut out = Vec::with_capacity(rows * hd);
                    for i in 0..rows {
                        let row = &src[i * stride + offset..][cols.clone()];
                        match bias {
                            Some(b) => out.extend(row.iter().zip(b).map(|(v, b)| v + b)),
                            None => out.extend_from_slice(row),
                        }
                    }
                    out
                };
            let qu = head(&qkv, 3 * d, 0, t, Some(&self.bias_u[cols.clone()]));
            let qv = head(&qkv, 3 * d, 0, t, Some(&self.bias_v[cols.clone()]));
            let ac = fgemm::matmul(&qu, t, hd, &head(&qkv, 3 * d, d, t, None), t);
            let bd = fgemm::matmul(&qv, t, hd, &head(r, d, 0, p, None), p);
            let mut probs = Buf::zeroed(t * t);
            for (i, row) in probs.chunks_exact_mut(t).enumerate() {
                // Column t-1-i+j of the position product is relative position i - j.
                let shifted = &bd[i * p + (t - 1 - i)..][..t];
                let mut max = f32::NEG_INFINITY;
                for j in 0..t {
                    row[j] = (ac[i * t + j] + shifted[j]) * scale;
                    max = max.max(row[j]);
                }
                let mut total = 0.0f32;
                for value in row.iter_mut() {
                    *value = (*value - max).exp();
                    total += *value;
                }
                let inv = 1.0 / total;
                row.iter_mut().for_each(|value| *value *= inv);
            }
            let mut v_t = Buf::zeroed(hd * t);
            for j in 0..t {
                for (e, value) in qkv[j * 3 * d + 2 * d..][cols.clone()].iter().enumerate() {
                    v_t[e * t + j] = *value;
                }
            }
            fgemm::matmul(&probs, t, t, &v_t, hd)
        });
        for (h, out) in per_head.iter().enumerate() {
            for i in 0..t {
                context[i * d + h * hd..i * d + (h + 1) * hd]
                    .copy_from_slice(&out[i * hd..(i + 1) * hd]);
            }
        }
        self.o.forward(&context, t)
    }

    /// Pointwise conv with GLU, depthwise conv over time, batch norm, SiLU, pointwise conv.
    fn convolution(&self, x: &[f32], t: usize, c: &Config) -> Buf {
        let d = c.hidden;
        let kernel = c.conv_kernel;
        let pad = (kernel - 1) / 2;
        let wide = self.pw1.forward(x, t);
        let mut gated = Buf::zeroed(t * d);
        team::chunks_mut(&mut gated, d, |index, out| {
            let row = &wide[index * 2 * d..(index + 1) * 2 * d];
            for ch in 0..d {
                out[ch] = row[ch] * sigmoid(row[d + ch]);
            }
        });
        let mut mixed = Buf::zeroed(t * d);
        team::chunks_mut(&mut mixed, d, |i, out| {
            for k in 0..kernel {
                let src = i + k;
                if src < pad || src - pad >= t {
                    continue;
                }
                let taps = &self.dw_by_tap[k * d..(k + 1) * d];
                for ((o, w), g) in out.iter_mut().zip(taps).zip(&gated[(src - pad) * d..][..d]) {
                    *o += w * g;
                }
            }
            for ((o, scale), shift) in out.iter_mut().zip(&self.bn_scale).zip(&self.bn_shift) {
                *o = silu(*o * scale + shift);
            }
        });
        self.pw2.forward(&mixed, t)
    }
}

/// Positions every take reaches without growing the cache: 30 s of 80 ms frames.
pub const POSITION_REACH: usize = 375;

/// Projected relative positions for `reach` down to `-reach`, one row each.
/// A row depends only on its position, never on the take's length or on
/// other rows, so one table serves every take up to `reach + 1` frames.
#[derive(Default)]
struct RelCache {
    table: RwLock<Option<Arc<RelRows>>>,
}

struct RelRows {
    reach: usize,
    rows: Buf,
}

impl RelRows {
    /// Rows for positions t-1 down to -(t-1).
    fn slice(&self, t: usize, d: usize) -> &[f32] {
        let first = self.reach - (t - 1);
        &self.rows[first * d..(first + 2 * t - 1) * d]
    }
}

impl RelCache {
    /// The table, grown first when `t` frames reach past it.
    fn rows(&self, t: usize, d: usize, project: impl Fn(&[f32], usize) -> Buf) -> Arc<RelRows> {
        if let Some(rows) = self.table.read().as_ref().filter(|r| r.reach + 1 >= t) {
            return Arc::clone(rows);
        }
        let mut table = self.table.write();
        if let Some(rows) = table.as_ref().filter(|r| r.reach + 1 >= t) {
            return Arc::clone(rows);
        }
        let reach = (t - 1)
            .max(POSITION_REACH)
            .max(table.as_ref().map_or(0, |r| 2 * r.reach));
        let positions = relative_positions(reach + 1, d);
        let rows = Arc::new(RelRows {
            reach,
            rows: project(&positions, 2 * reach + 1),
        });
        *table = Some(Arc::clone(&rows));
        rows
    }
}

/// Sinusoidal table for relative positions t-1 down to -(t-1), sin and cos interleaved.
fn relative_positions(t: usize, d: usize) -> Vec<f32> {
    let rows = 2 * t - 1;
    let inv_freq: Vec<f32> = (0..d / 2)
        .map(|i| 1.0 / 10000f32.powf((2 * i) as f32 / d as f32))
        .collect();
    let mut table = vec![0.0f32; rows * d];
    team::chunks_mut(&mut table, d, |p, row| {
        let position = (t - 1) as f32 - p as f32;
        for (i, freq) in inv_freq.iter().enumerate() {
            let angle = position * freq;
            row[2 * i] = angle.sin();
            row[2 * i + 1] = angle.cos();
        }
    });
    table
}

struct Lstm {
    /// `[W_ih | W_hh]`, applied to `[input | hidden]`.
    gates: HalfDense,
    bias: Vec<f32>,
}

pub struct DecoderState {
    hidden: Vec<Vec<f32>>,
    cell: Vec<Vec<f32>>,
}

pub struct Parakeet {
    pub config: Config,
    subsampling: Subsampling,
    blocks: Vec<Block>,
    encoder_projector: Dense,
    embedding: Vec<f32>,
    lstm: Vec<Lstm>,
    decoder_projector: HalfDense,
    joint: HalfDense,
}

impl Parakeet {
    pub fn load(t: &Tensors, config: Config, precision: Precision) -> Result<Self, String> {
        let c = &config;
        let dh = c.decoder_hidden;
        let blocks = team::map(c.layers, |index| Block::load(t, c, index, precision))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let lstm = (0..c.decoder_layers)
            .map(|l| -> Result<Lstm, String> {
                let b_ih = t.f32_len(&format!("decoder.lstm.bias_ih_l{l}"), 4 * dh)?;
                let b_hh = t.f32_len(&format!("decoder.lstm.bias_hh_l{l}"), 4 * dh)?;
                // LSTM weights carry no `.weight` suffix.
                let w_ih = t.f16_bits(&format!("decoder.lstm.weight_ih_l{l}"), 4 * dh * dh)?;
                let w_hh = t.f16_bits(&format!("decoder.lstm.weight_hh_l{l}"), 4 * dh * dh)?;
                let joined = HalfDense::side_by_side(&w_ih, &w_hh, 4 * dh, dh, dh);
                Ok(Lstm {
                    gates: HalfDense::new(joined, None, 4 * dh, 2 * dh)?,
                    bias: b_ih.iter().zip(&b_hh).map(|(a, b)| a + b).collect(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            subsampling: Subsampling::load(t, c)?,
            blocks,
            encoder_projector: dense(t, "encoder_projector", dh, c.hidden, true)?,
            embedding: t.f32_len("decoder.embedding.weight", c.vocab * dh)?,
            lstm,
            decoder_projector: half_dense(t, "decoder.decoder_projector", dh, dh)?,
            joint: half_dense(t, "joint.head", c.vocab + c.durations.len(), dh)?,
            config,
        })
    }

    /// Encoder output after the 24 blocks: `frames x hidden`.
    pub fn encode(&self, features: &[f32], frames: usize) -> (Buf, usize) {
        self.encode_inspect(features, frames, |_, _| {})
    }

    /// `inspect(0, x)` sees the subsampling output, `inspect(n, x)` the output of block n - 1.
    pub fn encode_inspect(
        &self,
        features: &[f32],
        frames: usize,
        mut inspect: impl FnMut(usize, &[f32]),
    ) -> (Buf, usize) {
        let c = &self.config;
        let (mut x, t) = self.subsampling.forward(features, frames, c.mels);
        inspect(0, &x);
        for (index, block) in self.blocks.iter().enumerate() {
            block.forward(&mut x, t, c);
            inspect(index + 1, &x);
        }
        (x, t)
    }

    /// Fills every layer's position table up front, so the first take does not wait.
    pub fn warm_positions(&self) {
        let d = self.config.hidden;
        for block in &self.blocks {
            block.rel_cache.rows(POSITION_REACH + 1, d, |table, rows| {
                block.rel_k.forward(table, rows)
            });
        }
    }

    pub fn project(&self, encoded: &[f32], t: usize) -> Buf {
        self.encoder_projector.forward(encoded, t)
    }

    fn decoder_step(&self, token: usize, state: &mut DecoderState) -> Buf {
        let dh = self.config.decoder_hidden;
        let mut input = self.embedding[token * dh..(token + 1) * dh].to_vec();
        let mut joined = vec![0.0f32; 2 * dh];
        for (layer, lstm) in self.lstm.iter().enumerate() {
            joined[..dh].copy_from_slice(&input);
            joined[dh..].copy_from_slice(&state.hidden[layer]);
            let mut gates = lstm.gates.matvec(&joined);
            for (g, b) in gates.iter_mut().zip(&lstm.bias) {
                *g += b;
            }
            // PyTorch gate order: input, forget, cell, output.
            let (h, c) = (&mut state.hidden[layer], &mut state.cell[layer]);
            for u in 0..dh {
                let ig = sigmoid(gates[u]);
                let fg = sigmoid(gates[dh + u]);
                let gg = gates[2 * dh + u].tanh();
                let og = sigmoid(gates[3 * dh + u]);
                c[u] = fg * c[u] + ig * gg;
                h[u] = og * c[u].tanh();
            }
            input = h.clone();
        }
        self.decoder_projector.matvec(&input)
    }

    /// Greedy TDT decoding over projected encoder frames (`t x decoder_hidden`).
    pub fn decode(&self, projected: &[f32], t: usize) -> Vec<usize> {
        let c = &self.config;
        let dh = c.decoder_hidden;
        let mut state = DecoderState {
            hidden: vec![vec![0.0; dh]; c.decoder_layers],
            cell: vec![vec![0.0; dh]; c.decoder_layers],
        };
        let mut dec = self.decoder_step(c.blank, &mut state);
        let mut tokens = Vec::new();
        let (mut frame, mut symbols) = (0usize, 0usize);
        let mut joint_in = vec![0.0f32; dh];
        while frame < t {
            let enc = &projected[frame * dh..(frame + 1) * dh];
            for i in 0..dh {
                joint_in[i] = (enc[i] + dec[i]).max(0.0);
            }
            let logits = self.joint.matvec(&joint_in);
            let token = argmax(&logits[..c.vocab]);
            let mut duration = c.durations[argmax(&logits[c.vocab..])];
            if token == c.blank {
                duration = duration.max(1);
            } else {
                tokens.push(token);
                dec = self.decoder_step(token, &mut state);
                symbols += 1;
                if duration == 0 && symbols >= c.max_symbols {
                    duration = 1;
                }
            }
            if duration > 0 {
                symbols = 0;
            }
            frame += duration;
        }
        tokens
    }
}

/// Index of the largest value; the first one wins a tie, as torch.argmax does.
fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in values.iter().enumerate() {
        if *v > values[best] {
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::{argmax, half, relative_positions, Config, RelCache, POSITION_REACH};
    use crate::stt::native::buf::Buf;

    #[test]
    fn cached_position_rows_equal_the_table_of_each_take() {
        let d = 8;
        let cache = RelCache::default();
        let copy = |table: &[f32], rows: usize| {
            let mut out = Buf::zeroed(rows * d);
            out.copy_from_slice(&table[..rows * d]);
            out
        };
        for t in [1, 2, 29, POSITION_REACH + 1, POSITION_REACH + 40, 3] {
            let rows = cache.rows(t, d, copy);
            assert_eq!(rows.slice(t, d), &relative_positions(t, d)[..], "t = {t}");
        }
    }

    #[test]
    fn the_shipped_config_shape_parses_and_others_are_refused() {
        let text = r#"{"model_type":"parakeet_tdt","blank_token_id":8192,"decoder_hidden_size":640,
            "durations":[0,1,2,3,4],"max_symbols_per_step":10,"num_decoder_layers":2,"vocab_size":8193,
            "ternary_group_size":128,"encoder_config":{"hidden_size":1024,"intermediate_size":4096,
            "num_attention_heads":8,"num_hidden_layers":24,"num_mel_bins":128,"conv_kernel_size":9,
            "subsampling_conv_channels":256,"subsampling_factor":8,"subsampling_conv_kernel_size":3,
            "subsampling_conv_stride":2,"scale_input":false}}"#;
        let config = Config::parse(text).expect("config");
        assert_eq!(
            (config.hidden, config.heads, config.durations.len()),
            (1024, 8, 5)
        );
        assert!(Config::parse(
            &text.replace("\"subsampling_factor\":8", "\"subsampling_factor\":4")
        )
        .is_err());
    }

    #[test]
    fn helpers_match_the_reference_definitions() {
        assert_eq!((half(614), half(307), half(154)), (307, 154, 77));
        assert_eq!(argmax(&[1.0, 3.0, 3.0, 2.0]), 1);
        let table = relative_positions(3, 4);
        // Row 0 is position +2: sin(2), cos(2), sin(2/100), cos(2/100).
        assert!((table[0] - 2f32.sin()).abs() < 1e-6 && (table[3] - 0.02f32.cos()).abs() < 1e-6);
        // Row 2 is position 0.
        assert_eq!(&table[8..12], &[0.0, 1.0, 0.0, 1.0]);
    }
}
