//! One speech model on disk, on Trit and the CPU. The model loads on first
//! use and can leave memory again.

use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::Instant;
use trit::{BiasWeights, LoadOptions, Precision, Trit};

/// Sends Trit's messages to the app log, once.
///
/// The engine runs Trit's first kernel set, which is Trit's default. It is
/// the only set with a speed that was measured on a phone (2026-11-02,
/// Snapdragon 8 Gen 3). The second set (`TRIT_KERNELS=v2`) gives the same
/// bits on every CPU, but its arm64 speed is not measured, and the text
/// changes with the operating system in each case (see docs/MAP.md).
fn send_log_to_app() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| trit::set_log(Some(Arc::new(crate::log::note))));
}

/// The user's terms, and whether the model in memory has them.
#[derive(Default)]
struct Dictionary {
    terms: Vec<String>,
    applied: bool,
}

pub struct Engine {
    model_dir: PathBuf,
    /// How Trit holds this model's weights.
    precision: Precision,
    /// Digital quiet after the audio of each read that ends a take, in
    /// samples. See `tdt_engine_new`.
    end_quiet: usize,
    native: Mutex<Option<Arc<Trit>>>,
    /// Held during a load, so a take that starts during a load waits for it
    /// and does not load a second model.
    loading: Mutex<()>,
    dictionary: Mutex<Dictionary>,
}

impl Engine {
    pub fn new(model_dir: &Path, precision: Precision, end_quiet: usize) -> Self {
        send_log_to_app();
        Self {
            model_dir: model_dir.to_path_buf(),
            precision,
            end_quiet,
            native: Mutex::new(None),
            loading: Mutex::new(()),
            dictionary: Mutex::new(Dictionary::default()),
        }
    }

    /// Digital quiet after the audio of each read that ends a take, in samples.
    pub fn end_quiet(&self) -> usize {
        self.end_quiet
    }

    /// The model, loaded now when it is not in memory.
    pub fn load(&self) -> Result<Arc<Trit>, String> {
        let _loading = self.loading.lock();
        if let Some(engine) = self.native.lock().clone() {
            return Ok(engine);
        }
        let started = Instant::now();
        // The cache maps the converted weights from a file next to the
        // model. They are then file memory: the system can drop their pages
        // when memory is short and read them again, so the model can stay
        // loaded. The first load writes the file.
        let options = LoadOptions {
            precision: self.precision,
            cache: true,
            ..LoadOptions::default()
        };
        let engine = Arc::new(Trit::load_with(&self.model_dir, &options)?);
        *self.native.lock() = Some(Arc::clone(&engine));
        // Loading converts weights through buffers that are freed again.
        trit::release_memory();
        self.remove_other_caches();
        // A new model has no dictionary yet.
        self.dictionary.lock().applied = false;
        crate::log::note(&format!(
            "loaded {} ({}, {}) in {} ms",
            engine.kind(),
            self.precision.name(),
            trit::cpu_path(),
            started.elapsed().as_millis()
        ));
        Ok(engine)
    }

    /// Deletes the weight caches of other precisions. Trit keeps them for
    /// other programs, but on a phone only this app reads the folder, and a
    /// cache of Parakeet Unified is about 800 MB.
    fn remove_other_caches(&self) {
        // `weights-<Trit version>-<precision>-<kernel paths>.bin`
        let own = format!("weights-{}-{}-", trit::VERSION, self.precision.name());
        let Ok(entries) = std::fs::read_dir(self.model_dir.join(".trit")) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("weights-") && name.ends_with(".bin") && !name.starts_with(&own) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    /// Frees the model. A take that runs keeps its model until it ends.
    pub fn unload(&self) {
        let _loading = self.loading.lock();
        *self.native.lock() = None;
        trit::release_memory();
    }

    /// Terms to favor, as the user wrote them. The next take applies them.
    pub fn set_dictionary(&self, terms: Vec<String>) {
        let mut dictionary = self.dictionary.lock();
        if dictionary.terms != terms {
            dictionary.terms = terms;
            dictionary.applied = false;
        }
    }

    /// Gives `engine` the user's terms when it does not have them.
    pub(crate) fn refresh_dictionary(&self, engine: &Trit) {
        let mut dictionary = self.dictionary.lock();
        if dictionary.applied {
            return;
        }
        dictionary.applied = true;
        let skipped = engine.set_dictionary(&dictionary.terms, BiasWeights::default());
        if !skipped.is_empty() {
            // The count only: the log holds no text of the user.
            crate::log::note(&format!(
                "The model cannot spell {} of the {} dictionary terms. TDT does not use them.",
                skipped.len(),
                dictionary.terms.len()
            ));
        }
    }
}
