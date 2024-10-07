# Demo with localnode

This shows how to deploy an AVS on localnode and setup stuff

## Start localnode

If you want to compile the most recent versions, try this in this repo, as well as
the [wasmatic repo](https://github.com/Lay3rLabs/wasmatic):

```bash
./scripts/build_docker.sh
```

Otherwise, you can just get the most recent published images here:

```bash
cd localnode
docker compose pull
```

Once the images are updated, reset everything and start fresh:

```bash
cd localnode
./reset_volumes.sh
./run.sh
```

The rest of this document assumes you have a well-running instance here.

### Check Localnode

```bash
# ensure cometbft, gateway, jaegertracing, slay3r, and wasmatic containers are running
docker ps

curl localhost:26657/status
# see blocks are being made.. run a few times
curl localhost:26657/status | jq .result.sync_info

# ensure wasmatic has some proper operator addresses
curl localhost:8081/info | jq .
```

## Deploy AVS stuff

More info available on the [README there](https://github.com/Lay3rLabs/example-avs-oracle/blob/main/tools/cli/README.md).

### Set Up Wallet

```bash
M=$(cargo run wallet create | tail -1 | cut -c16-)
echo "LOCAL_MNEMONIC=\"$M\"" > .env
# this should have a nice 24 word phrase
cat .env

cargo run -- --target=local faucet tap
cargo run -- --target=local wallet show
```

### Deploy Contracts

```bash
# rebuild the contracts locally
(cd ../.. && ./scripts/optimizer.sh)

# deploy them
cargo run -- --target=local deploy contracts --operators wasmatic

# Copy the line that says "export LOCAL_TASK_QUEUE_ADDRESS" and paste it in your shell

# make sure we set this up properly
cargo run -- --target=local task-queue view-queue
```

### Deploy WASI component

In order to make use of the oracle example, you need to obtain the CoinGecko API key first. Please visit [this page](https://docs.coingecko.com/reference/setting-up-your-api-key) for more informations.

```bash
# rebuild the component
(cd ../.. && ./scripts/build_wasi.sh)

# Testable is optional if you want to try the next step
# Do not use for production deployments
cargo run -- --target=local wasmatic deploy --name demo1 \
    --wasm-source ../../components/oracle_example.wasm  \
    --testable \
    --envs "API_KEY=<YOUR_COINGECKO_API_KEY>" \
    --task $LOCAL_TASK_QUEUE_ADDRESS
```

Now, you can check it is installed and running properly by checking
the wasmatic logs (in another console ideally):

```bash
docker logs -f localnode-wasmatic-1
```

### Test a Component

This can only be done if `--testable` was provided above

```bash
cargo run -- --target=local wasmatic test --name demo1 --input '{}'
```

It will parse the input as if you pushed it to the task queue and return
the result (or error) to the caller. Nothing is written on chain.

Note: if you change state when being triggered, this will break the AVS
consensus mechanism (different results for different operators), and thus
should not be used in production.

### Trigger Task

```bash
cargo run -- --target=local task-queue view-queue

cargo run -- --target=local task-queue add-task -b '{}' -d 'test 1'

# wait a few secords, or until the log output shows it is executed
cargo run -- --target=local task-queue view-queue
```


