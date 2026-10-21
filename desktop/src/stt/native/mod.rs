//! TDT's own Parakeet Redux engine: no Python, no external runtime.
//! The model files are read as published (CC-BY-4.0); the network follows
//! the Apache-2.0 Hugging Face Parakeet implementation.

#[macro_use]
mod simd;

mod activation;
mod bias;
mod buf;
mod features;
mod fgemm;
mod gemm;
mod half;
mod kernels;
mod model;
mod team;
mod tensors;
mod tokenizer;

use parking_lot::RwLock;
use std::path::Path;

pub use bias::BiasWeights;

pub use gemm::Precision;
pub use model::Config;

pub struct NativeParakeet {
    model: model::Parakeet,
    /// The user's dictionary as token paths; empty means plain greedy decoding.
    dictionary: RwLock<Option<bias::Bias>>,
    frontend: features::MelFrontend,
    tokenizer: tokenizer::Tokenizer,
}

impl NativeParakeet {
    pub fn load(model_dir: &Path) -> Result<Self, String> {
        Self::load_with(model_dir, Precision::Int8)
    }

    pub fn load_with(model_dir: &Path, precision: Precision) -> Result<Self, String> {
        let config_text = std::fs::read_to_string(model_dir.join("config.json"))
            .map_err(|e| format!("Could not read config.json: {e}"))?;
        let config = Config::parse(&config_text)?;
        // One thread per physical core of one L3 domain. A second thread on a
        // core shares its vector units and only adds contention.
        let (threads, domain) = engine_threads();
        team::init(threads, domain);
        let tensors = tensors::Tensors::open(&model_dir.join("model.safetensors"))?;
        let frontend = features::MelFrontend::new(config.mels);
        let model = model::Parakeet::load(&tensors, config, precision)?;
        // The 178 MB file is no longer needed; free it before the position
        // tables grow, so load does not hold both at its peak.
        drop(tensors);
        if precision == Precision::Int8 {
            model.warm_positions();
        }
        Ok(Self {
            dictionary: RwLock::new(None),
            model,
            frontend,
            tokenizer: tokenizer::Tokenizer::open(&model_dir.join("tokenizer.json"))?,
        })
    }

    /// Terms to favor while decoding, as written ("Kubernetes", "PostgreSQL").
    /// Returns the terms the vocabulary cannot spell; they are skipped.
    pub fn set_dictionary(&self, terms: &[String], weights: BiasWeights) -> Vec<String> {
        let mut paths = Vec::new();
        let mut skipped = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for term in terms.iter().map(|t| t.trim()).filter(|t| !t.is_empty()) {
            let Some(written) = self.tokenizer.encode(term) else {
                skipped.push(term.to_string());
                continue;
            };
            for (variant, rewrite) in bias::variants(term) {
                let Some(path) = self.tokenizer.encode(&variant) else {
                    continue;
                };
                if seen.insert(path.clone()) {
                    paths.push(bias::Term {
                        written: (path != written).then(|| written.clone()),
                        always: rewrite,
                        path,
                    });
                }
            }
        }
        let letters: Vec<usize> = (0..self.tokenizer.len())
            .map(|id| self.tokenizer.letters(id))
            .collect();
        let bias = bias::Bias::new(paths, &letters, self.tokenizer.word_starts(), weights);
        *self.dictionary.write() = (!bias.is_empty()).then_some(bias);
        skipped
    }

    /// 16 kHz mono samples to text.
    pub fn transcribe(&self, samples: &[f32]) -> String {
        let tokens = self.tokens(samples);
        self.tokenizer.decode(&tokens)
    }

    pub fn tokens(&self, samples: &[f32]) -> Vec<usize> {
        // The calling thread works on every step, so it joins the team's L3 domain.
        let _pinned = team::pin_caller();
        let (features, frames) = self.frontend.compute(samples);
        // The subsampling needs at least one output frame per stage.
        if frames < 8 {
            return Vec::new();
        }
        let (encoded, t) = self.model.encode(&features, frames);
        let projected = self.model.project(&encoded, t);
        let dictionary = self.dictionary.read();
        self.model.decode(&projected, t, dictionary.as_ref())
    }
}

/// Team size and the affinity mask of its L3 domain (0: no pinning).
/// `TDT_NATIVE_THREADS` overrides the physical core count and turns pinning off.
fn engine_threads() -> (usize, usize) {
    if let Some(threads) = std::env::var("TDT_NATIVE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|t| *t > 0)
    {
        return (threads, 0);
    }
    physical_cores().unwrap_or_else(|| {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        (threads, 0)
    })
}

/// Physical cores that share one L3 cache: 8 on a two-CCD Ryzen, all cores
/// on a CPU with one shared L3. Threads on another L3 domain pay for every
/// exchange of the small per-layer matrices, so with several domains the
/// team stays on one: the one with the most cores, then the largest L3
/// (the V-cache CCD of an X3D part).
/// `cores` holds one processor mask per physical core, `l3` a mask and a size per L3 cache.
fn pick_domain(cores: &[usize], l3: &[(usize, u32)]) -> Option<(usize, usize)> {
    let count = |domain: usize| cores.iter().filter(|core| *core & domain != 0).count();
    let best = l3
        .iter()
        .max_by_key(|(domain, size)| (count(*domain), *size))
        .filter(|(domain, _)| count(*domain) > 0);
    match best {
        Some((domain, _)) if l3.len() > 1 => Some((count(*domain), *domain)),
        Some((domain, _)) => Some((count(*domain), 0)),
        None => Some((cores.len(), 0)).filter(|(n, _)| *n > 0),
    }
}

#[cfg(windows)]
fn physical_cores() -> Option<(usize, usize)> {
    use windows::Win32::System::SystemInformation::{
        CacheUnified, GetLogicalProcessorInformation, RelationCache, RelationProcessorCore,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };
    let mut bytes = 0u32;
    // SAFETY: the first call only reports the buffer size.
    let _ = unsafe { GetLogicalProcessorInformation(None, &mut bytes) };
    let count = bytes as usize / std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    if count == 0 {
        return None;
    }
    let mut info = vec![SYSTEM_LOGICAL_PROCESSOR_INFORMATION::default(); count];
    // SAFETY: the buffer holds `bytes` bytes, as the first call asked.
    unsafe { GetLogicalProcessorInformation(Some(info.as_mut_ptr()), &mut bytes) }.ok()?;
    let cores: Vec<usize> = info
        .iter()
        .filter(|entry| entry.Relationship == RelationProcessorCore)
        .map(|entry| entry.ProcessorMask)
        .collect();
    // SAFETY: `Cache` is the active union member for RelationCache entries.
    let l3: Vec<(usize, u32)> = info
        .iter()
        .filter(|entry| entry.Relationship == RelationCache)
        .filter_map(|entry| unsafe {
            let cache = entry.Anonymous.Cache;
            (cache.Level == 3 && cache.Type == CacheUnified)
                .then_some((entry.ProcessorMask, cache.Size))
        })
        .collect();
    pick_domain(&cores, &l3)
}

#[cfg(not(windows))]
fn physical_cores() -> Option<(usize, usize)> {
    None
}
