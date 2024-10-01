# Simple AVS Oracle example

This component queries the CoinGecko API with a configured `API_KEY` env variable.
Tracks the recent BTCUSD prices and computes the average price price over the past
minute and hour.

## Setup

This requires Rust 1.80+. Please ensure you have that installed via `rustup`
before continuing.

Even though we will be building a Wasm component that targets WASI Preview 2, the Rust
`wasm32-wasip2` target is not quite ready yet. So we will use `cargo-component` to target
`wasm32-wasip1` and package to use WASI Preview 2.

If haven't yet, add the WASI Preview 1 target:
```
rustup target add wasm32-wasip1
```

Install `cargo-component`:
```
cargo install cargo-component
```

The configuration for registry mappings is in the process of getting better,
but for now, it is manual.

The default location is `$XDG_CONFIG_HOME/wasm-pkg/config.toml` on unix-like systems and
`{FOLDERID_RoamingAppData}\wasm-pkg\config.toml` on Windows. Examples of this are found below:

| Platform | Path                                            |
| -------- | ----------------------------------------------- |
| Linux    | `/home/<username>/.config`                      |
| macOS    | `/Users/<username>/Library/Application Support` |
| Windows  | `C:\Users\<username>\AppData\Roaming`           |

The configuration file is TOML and currently must be edited manually. A future release will include
an interactive CLI for editing the configuration. For more information about configuration, see
the [wkg docs](https://github.com/bytecodealliance/wasm-pkg-tools).

The recommended configuration that will work out of the box:

```toml
default_registry = "wa.dev"
```

## Usage

On your CLI, navigate to this directory, then run:
```
cargo component build --release
```

This produces a Wasm component bindary that can be found 
in the workspace target directory (`../../target/wasm32-wasip1/release/oracle_example.wasm`).
