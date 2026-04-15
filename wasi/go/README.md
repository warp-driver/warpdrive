# warpdrive-go

[WarpDrive](https://wavs.xyz) Go bindings for WASI [components](https://github.com/Lay3rLabs/wavs-foundry-template) — lets you write Vectr circuits in Go instead of Rust.

## Install Wit Bindgen for Go

```bash
go install go.bytecodealliance.org/cmd/wit-bindgen-go@ecfa620df5beee882fb7be0740959e5dfce9ae26

wit-bindgen-go --version
```

## System Setup

```bash
# https://component-model.bytecodealliance.org/language-support/go.html

# https://tinygo.org/getting-started/install/

# macOS
brew tap tinygo-org/tools
brew install tinygo

# Arch (btw)
sudo pacman -Sy tinygo

# Ubuntu / WSL:
# TODO: .
```

## Generate Bindings

```bash
# verify installs
tinygo version
wkg --version

# build the warpdrive package if you have not already
wkg wit build

# move into the golang directory
cd go/

# generate the Go/ bindings
# if `error: error executing wasm-tools: module closed with exit_code(1)`, set WARPDRIVE_PACKAGE
wit-bindgen-go generate -o . ../wit-definitions/operator/wavs:operator@0.6.0-alpha.6.wasm

go mod tidy
```
