# Development Guidelines

## Before committing

Always run the following checks and ensure they pass:

```sh
cargo fmt --check
cargo clippy --target x86_64-unknown-uefi
```

## Build

```sh
cargo build --target x86_64-unknown-uefi
```
