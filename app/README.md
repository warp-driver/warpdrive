# WarpDrive Desktop App

Tauri 2 + React 19 + TypeScript desktop client for running a local WarpDrive node (Vectr). Bridges to the `warpdrive` Rust runtime via `wavs_app_lib`, wires up a Zustand store for state, and uses Viem for EVM interaction.

## Dev

```bash
just app-dev            # Full Tauri dev with hot reload
just app-dev-frontend   # Vite frontend dev server only
just app-build-release  # Release build
```

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
