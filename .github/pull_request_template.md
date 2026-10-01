## Summary

<!-- Describe what changed and why. -->

## Linked Issue

<!-- Use a closing keyword when possible, for example: Closes #123. -->

Closes #

## Testing

<!-- List the commands you ran, or explain why testing was not run. -->

## Checklist

- [ ] `cargo audit --deny warnings`
- [ ] `cargo fmt --check`
- [ ] `cargo clippy --locked --all-targets -- -D warnings`
- [ ] `cargo test --locked`
- [ ] `cargo build --locked --release --target wasm32v1-none`
