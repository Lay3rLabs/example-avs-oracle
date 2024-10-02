# Simple AVS Oracle example

This component queries the CoinGecko API with a configured `API_KEY` env variable.
Tracks the recent BTCUSD prices and returns the average price price over the past
hour.

## Setup

This requires Rust 1.80+. Please ensure you have that installed via `rustup`
before continuing.

Even though we will be building a Wasm component that targets WASI Preview 2, the Rust
`wasm32-wasip2` target is not quite ready yet. So we will use
[`cargo-component`](https://github.com/bytecodealliance/cargo-component) to target
`wasm32-wasip1` and package to use WASI Preview 2.

If haven't yet, add the WASI Preview 1 target:
```bash
rustup target add wasm32-wasip1
```

Install `cargo-component` and `wkg` CLIs:
```bash
cargo install cargo-component wkg
```

Set default registry configuration:
```bash
wkg config --default-registry wa.dev
```
For more information about configuration, see
the [wkg docs](https://github.com/bytecodealliance/wasm-pkg-tools).

## Build

On your CLI, navigate to this directory, then run:
```bash
cargo component build --release
```

This produces a Wasm component bindary that can be found 
in the workspace target directory (`../target/wasm32-wasip1/release/oracle_example.wasm`).

Optionally, run `cargo fmt` to format the source and generated files before commiting the code.

## Unit Testing

To run the unit tests, build the component first with:
```bash
cargo component build
```
and then:
```bash
cargo test
```

## Deploy

Upload the compiled Wasm component to the Wasmatic node.
```
curl -X POST --data-binary @../target/wasm32-wasip1/release/oracle_example.wasm http://localhost:8081/upload
```

Copy the digest SHA returned and replace the placeholder `{DIGEST}` in the below `curl` command.

This example integrates with the CoinGecko API to retrieve the latest BTCUSD price. You will need to sign up
and provide an API key, [see instructions](https://docs.coingecko.com/reference/setting-up-your-api-key).
Replace the `{API_KEY}` below with your key.

Choose a unique application name string and use in the placeholder `{PLACEHOLDER-UNIQUE-NAME}` below `curl` commands.

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
  "envs": [
    ["API_KEY", "{API_KEY}"]
  ],
  "testable": true
}
EOF

curl -X POST -H "Content-Type: application/json" http://localhost:8081/app -d "$BODY"
```

## Testing Deployment

To test the deployed application on the Wasmatic node, you can use the test endpoint.
The server responds with the output of the applicaton without sending the result to the chain.


```bash
curl --request POST \
  --url http://localhost:8081/test \
  --header 'Content-Type: application/json' \
  --data '{
  "name": "{PLACEHOLDER-UNIQUE-NAME}",
  "input": {}
}'
```
