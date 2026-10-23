//! Downloads a speech model the way Settings does, from a terminal, with no
//! script to allow: `cargo run --release --bin download-model [-- <id>]`.
//! The default id is the default model (Parakeet Redux). Each file is checked
//! against its SHA-256 before it is kept.

use std::io::Write;
use voice_stt_desktop::stt::models::{self, CATALOG};

fn main() -> std::process::ExitCode {
    let id = std::env::args().nth(1);
    let Some(spec) = id.as_deref().map_or(Some(models::DEFAULT), models::by_id) else {
        let ids: Vec<&str> = CATALOG.iter().map(|spec| spec.id).collect();
        eprintln!("error: unknown model; valid ids: {}", ids.join(", "));
        return std::process::ExitCode::from(2);
    };
    let dest = models::install_dir(spec);
    println!("{} ({}) to {}", spec.label, spec.size_label, dest.display());
    let mut shown = u8::MAX;
    let result = models::download(spec, &dest, |progress| {
        let percent = models::percent(progress.done, progress.total);
        if percent != shown {
            shown = percent;
            print!("\r{percent:3}% {}", progress.file);
            let _ = std::io::stdout().flush();
        }
    });
    println!();
    match result {
        Ok(()) => {
            println!("{} is ready in {}", spec.label, dest.display());
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
