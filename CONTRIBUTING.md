# Contributing

1. Install Rust stable.
2. Download the model with `cargo run --release --bin download-model --manifest-path desktop/Cargo.toml`.
3. Before opening a change, run:

```powershell
cargo fmt --manifest-path desktop/Cargo.toml -- --check
cargo clippy --manifest-path desktop/Cargo.toml --all-targets --all-features -- -D warnings
cargo clippy --manifest-path packaging/tdt-setup/Cargo.toml --all-targets -- -D warnings
```

4. To build `dist/TDT-Setup.exe`: `powershell -ExecutionPolicy Bypass -File packaging/build-installer.ps1`.

## Release

Set the version in `desktop/Cargo.toml` and push a matching tag, such as
`v0.7.0`. The `Release` workflow builds `TDT-Setup.exe` and attaches it with
`SHA256SUMS.txt`. TDT 0.2.32 and older need `SHA256SUMS.txt` to find the
update.

## Build with a local Trit

TDT uses a tagged version of [Trit](https://github.com/Hi9841/trit)
(`desktop/Cargo.toml`). To build TDT with a local Trit checkout:

```powershell
cargo build --release --manifest-path .\desktop\Cargo.toml --config "patch.'https://github.com/Hi9841/trit'.trit.path='..\trit'"
```

## Repository layout

| Folder | Contents |
|---|---|
| `desktop/` | The Windows app (Rust and GPUI) and the speech benchmark. |
| `packaging/` | `TDT-Setup.exe` (install, update, uninstall) and its build script. |
| `docs/` | The README images. `docs/images/capture.py` captures the app pictures again. |
| `site/` | The TDT website. |

## Pull requests

Keep model weights and generated build directories out of commits. Describe
user-visible behavior and include the checks you ran in pull requests.
By contributing you agree the change is licensed under the PolyForm
Noncommercial License 1.0.0 (see LICENSE).
