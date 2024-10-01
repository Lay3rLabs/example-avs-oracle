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

## Build

On your CLI, navigate to this directory, then run:
```
cargo component build --release
```

This produces a Wasm component bindary that can be found 
in the workspace target directory (`../target/wasm32-wasip1/release/oracle_example.wasm`).

## Deploy

Upload the compiled Wasm component to the Wasmatic node.
```
curl -X POST --data-binary @../target/wasm32-wasip1/release/oracle_example.wasm http://0.0.0.0:8081/upload
```

Copy the digest SHA returned.
Choose a unique application name string and use in the placeholder below CURL commands.

```
read -d '' BODY << "EOF"
{
  "name": "{PLACEHOLDER-UNIQUE-NAME}",
  "digest": "sha256:{DIGEST}",
  "trigger": {
    "queue": {
      "taskQueueAddr": "{TASK-QUEUE-ADDR}",
      "hdIndex": 1,
      "pollInterval": 5
    }
  },
  "permissions": {},
  "envs": []
}
EOF

curl -X POST -H "Content-Type: application/json" http://0.0.0.0:8081/app -d "$BODY"
```
