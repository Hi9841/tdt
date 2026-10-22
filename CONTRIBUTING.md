# Contributing

1. Install Rust stable.
2. Download the model with `powershell -ExecutionPolicy Bypass -File models/download-models.ps1`.
3. Before opening a change, run:

```powershell
cargo fmt --manifest-path desktop/Cargo.toml -- --check
cargo clippy --manifest-path desktop/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path packaging/tdt-setup/Cargo.toml --all-targets -- -D warnings
```

4. To build `dist/TDT-Setup.exe`: `powershell -ExecutionPolicy Bypass -File packaging/build-installer.ps1`.

Keep model weights and generated build directories out of commits. Describe
user-visible behavior and include the checks you ran in pull requests.
By contributing you agree the change is licensed under the PolyForm
Noncommercial License 1.0.0 (see LICENSE).
