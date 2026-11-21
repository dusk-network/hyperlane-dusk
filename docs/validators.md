# Validators

Sepolia accepts a message from Dusk once enough Dusk-origin validators have
signed it: `DUSK_ORIGIN_THRESHOLD` of the `DUSK_ORIGIN_VALIDATORS`. Each
validator watches the Dusk Merkle tree hook through a Dusk node, signs a
checkpoint for every message, and writes the checkpoints to a bucket anyone
can read. The relayer collects the signatures from there. Sepolia → Dusk
works the same way with `EVM_ORIGIN_VALIDATORS`.

This page is for running a Dusk-origin validator, and for adding one to the
set. [testnet.md](testnet.md) covers the deployment itself.

## Run a validator

A validator holds one key: an EVM key it signs checkpoints with. It sends no
transactions and needs no DUSK or ETH. There's no reward for running one:
on testnet you run it so the bridge depends on more than our two.

You need:

- A Linux host with Docker, or Linux x86-64 with glibc 2.39 or later
  (Ubuntu 24.04, Debian 13) for the binary. Our validators reserve 300 MHz
  of CPU and 256 MB of memory, may use up to 1 GB, and need a little disk
  for their database.
- A key of its own. Generate one with `cast wallet new` (Foundry), or any
  Ethereum wallet tool. It needs no funds. Keep the private key on the
  validator's host: we only need its address.
- The env file we send you, `dusk-validator.env`, which names your bucket and
  holds its credentials.

Put the key in a file only you can read:

```bash
umask 077
printf 'HYP_VALIDATOR_KEY=%s\n' '<private key, 0x...>' > validator-key.env
```

Then run the image from the release:

```bash
docker run -d --name dusk-validator --restart unless-stopped \
  --env-file dusk-validator.env --env-file validator-key.env \
  -v dusk-validator:/data -p 127.0.0.1:9091:9091 \
  ghcr.io/dusk-network/hyperlane-dusk-validator:<version>
```

Or the binary, from the same release, with the config next to it:

```bash
tar xzf hyperlane-dusk-validator-<version>-linux-x86_64.tar.gz
cd hyperlane-dusk-validator-<version>
set -a; . ../dusk-validator.env; . ../validator-key.env; set +a
CONFIG_FILES="$PWD/dusk-testnet-validator.json" ./validator
```

The binary keeps its state under `/data`, as the image does: create it, owned
by the user that runs it, or set `HYP_DB` and
`HYP_CHAINS_DUSK_EVENTCURSORDIR` to directories of your own.

Send us the validator's address (`cast wallet address --private-key <key>`).
On startup it writes its signed announcement to your bucket, then waits,
logging `Cannot announce validator without a signer` every few seconds,
until we announce it on Dusk for it. Then it logs `Validator has announced
signature storage location` and starts signing.

To check it's working:

- `curl -s 127.0.0.1:9091/metrics | grep hyperlane_latest_checkpoint` shows
  the index it has signed up to, which follows the Dusk Merkle tree hook.
- `checkpoint_latest_index.json` in your bucket says the same.

What the key can and can't do:

- It signs checkpoints. With enough other validators' signatures, a
  checkpoint lets the relayer deliver a message to Sepolia, so keep the key
  secret. A stolen key alone, below the threshold, can't get anything
  accepted.
- It holds no funds and can't move any.
- If it leaks, or the host is compromised, tell us: we take it out of the set.

Restarts are fine; it picks up where it stopped. Tell us before you stop for
good, so we take it out of the set first: a validator the threshold counts
on that stops signing stalls transfers.

## Add a validator

For the deployment's owner. Dusk-origin validators without a Dusk account
can't announce themselves, so `announce` sends their signed announcement
for them, from `DUSK_KEYS`'s account. The contract checks the validator's
signature, not who sends it.

1. Give the operator a bucket: on Hetzner Object Storage, in `nbg1`, behind
   `CHECKPOINT_S3_ENDPOINT`, in a project of its own (S3 credentials cover
   every bucket in their project). Its policy must let anyone get and list
   its objects, as ours does. Send the operator `dusk-validator.env`:

   ```bash
   HYP_CHECKPOINTSYNCER_BUCKET=<bucket>
   HYP_CHECKPOINTSYNCER_REGION=nbg1
   HYP_CHECKPOINTSYNCER_FOLDER=<operator>
   AWS_ACCESS_KEY_ID=<key>
   AWS_SECRET_ACCESS_KEY=<secret>
   AWS_ENDPOINT_URL=https://nbg1.your-objectstorage.com
   ```

2. Once it runs, announce it:

   ```bash
   bash deploy/testnet.sh announce s3://<bucket>/nbg1/<operator> <its address>
   ```

   This reads its announcement from the bucket, checks it is that
   validator's, its signature, the Mailbox and the domain, and records the
   location in the deployment's state. Run it once for each validator already in the set too: for one
   that announced itself, it only records where.

3. If the new threshold needs more gas on Sepolia, raise
   `DUSK_IGP_GAS_OVERHEAD` first (`fees` and `preflight` say), and apply it
   with `dusk-igp --apply`.

4. Add the validator to `DUSK_ORIGIN_VALIDATORS`, set
   `DUSK_ORIGIN_THRESHOLD`, and check:

   ```bash
   bash deploy/testnet.sh set-validators
   ```

   It refuses unless every validator in the new set is announced and has
   signed a checkpoint for one of the last `VALIDATOR_CHECKPOINT_LAG`
   messages, signed by it and for this deployment's tree. Then switch:

   ```bash
   bash deploy/testnet.sh set-validators --apply
   ```

   Sepolia's MessageIdMultisig ISMs can't be changed, so this deploys one for
   the new set through `EVM_ISM_FACTORY` and points every route's token at
   it. The relayer reads the ISM from the token, so it needs no new config.

Adding validators at a threshold our own two still meet keeps transfers
going while a new one settles in. The threshold has to exceed the
validators we run before outside validators add security: 3 of 4 with two
of ours and two outside.

`EVM_ORIGIN_VALIDATORS` and `EVM_ORIGIN_THRESHOLD` change the same way:
`set-validators` sets them on the Dusk ISMs the routes use. EVM-origin
validators announce themselves on Sepolia, as any Hyperlane validator does.
A higher threshold needs a higher `DUSK_RELAYER_GAS_LIMIT`: regenerate the
agent configs with it (`agent-configs`) and restart the relayer before you
switch.
