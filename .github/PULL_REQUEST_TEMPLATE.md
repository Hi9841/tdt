## Change

-

## Checks

- [ ] `cargo fmt --manifest-path desktop/Cargo.toml -- --check`
- [ ] `cargo test --manifest-path desktop/Cargo.toml`
- [ ] `cargo clippy --manifest-path desktop/Cargo.toml --all-targets --all-features -- -D warnings`

Do not commit model weights or `desktop/target/`.
