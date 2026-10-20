//! TDT's own Parakeet Redux engine: no Python, no external runtime.
//! The model files are read as published (CC-BY-4.0); the network follows
//! the Apache-2.0 Hugging Face Parakeet implementation.

mod features;
mod gemm;
mod model;
mod tensors;
mod tokenizer;

use std::path::Path;

pub use model::Config;

pub struct NativeParakeet {
    model: model::Parakeet,
    frontend: features::MelFrontend,
    tokenizer: tokenizer::Tokenizer,
}

impl NativeParakeet {
    pub fn load(model_dir: &Path) -> Result<Self, String> {
        let config_text = std::fs::read_to_string(model_dir.join("config.json"))
            .map_err(|e| format!("Could not read config.json: {e}"))?;
        let config = Config::parse(&config_text)?;
        let tensors = tensors::Tensors::open(&model_dir.join("model.safetensors"))?;
        let frontend = features::MelFrontend::new(config.mels);
        let model = model::Parakeet::load(&tensors, config)?;
        Ok(Self {
            model,
            frontend,
            tokenizer: tokenizer::Tokenizer::open(&model_dir.join("tokenizer.json"))?,
        })
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
        self.model.decode(&projected, t)
    }
}

#[cfg(test)]
mod reference_tests;
