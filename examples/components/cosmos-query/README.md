## Start local chain

```bash
starship start --config ./starship.yaml
```

## (Re)build component

In repo root:

```bash
just wasi-build
```

## Run via WarpDrive test

In repo `packages/wavs`:

```bash
RUST_LOG="info,wavs=debug" cargo test --workspace -- --nocapture
```

## Stop local chain

```bash
starship stop --config ./starship.yaml
```
