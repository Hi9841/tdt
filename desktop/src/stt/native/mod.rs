//! TDT's own Parakeet Redux engine: no Python, no external runtime.
//! The model files are read as published (CC-BY-4.0); the network follows
//! the Apache-2.0 Hugging Face Parakeet implementation.

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
        team::init(engine_threads());
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

/// `TDT_NATIVE_THREADS` overrides the physical core count.
fn engine_threads() -> usize {
    if let Some(threads) = std::env::var("TDT_NATIVE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|t| *t > 0)
    {
        return threads;
    }
    physical_cores().unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
}

/// Physical cores that share one L3 cache: 8 on a two-CCD Ryzen, all cores
/// on a CPU with one shared L3. Threads on another L3 domain pay for every
/// exchange of the small per-layer matrices.
#[cfg(windows)]
fn physical_cores() -> Option<usize> {
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
    let l3_domains: Vec<usize> = info
        .iter()
        .filter(|entry| entry.Relationship == RelationCache)
        .filter(|entry| unsafe {
            entry.Anonymous.Cache.Level == 3 && entry.Anonymous.Cache.Type == CacheUnified
        })
        .map(|entry| entry.ProcessorMask)
        .collect();
    let per_domain = l3_domains
        .iter()
        .map(|domain| cores.iter().filter(|core| *core & domain != 0).count())
        .max()
        .unwrap_or(0);
    let total = cores.len();
    Some(if per_domain > 0 { per_domain } else { total }).filter(|n| *n > 0)
}

#[cfg(not(windows))]
fn physical_cores() -> Option<usize> {
    None
}

#[cfg(test)]
mod reference_tests;

#[cfg(test)]
mod thread_tests {
    #[test]
    fn the_core_count_is_plausible() {
        let threads = super::engine_threads();
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        println!("engine threads {threads}, logical {logical}");
        assert!(threads >= 1 && threads <= logical);
    }
}
