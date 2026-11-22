# Validators

Sepolia accepts a message from Dusk once `DUSK_ORIGIN_THRESHOLD` of the
`DUSK_ORIGIN_VALIDATORS` have signed it. Each validator watches the Dusk
Merkle tree hook through a Dusk node, signs a checkpoint for every message,
and writes the checkpoints to a bucket anyone can read. The relayer collects
the signatures from there.

Ethereum → Dusk trusts Hyperlane's Ethereum default set (6 of 9 when
written; `typescript/sdk/src/consts/multisigIsm.ts` upstream). We mirror
their changes into our ISM with `dusk-tx set-ism-validators`, and should
monitor for differences. The focus is an independent Dusk-origin set: the
weaker direction bounds the route, since a forged Dusk message mints wDUSK
that can be bridged back against the DUSK we hold. The relayer must read AWS
S3 buckets for that (upstream #9159, or our validators off the global
endpoint override). An aggregation ISM with a set of our own is an option
later, not needed at launch.

## Run a validator

A validator holds one key, an EVM key it signs checkpoints with. It sends no
transactions and needs no DUSK or ETH. There's no reward for running one: on
testnet you run it so the bridge depends on more than our two.

You need:

- A Linux host with Docker, or Linux x86-64 with glibc 2.39 or later
  (Ubuntu 24.04, Debian 13) for the binary. Our validators reserve 300 MHz
  of CPU and 256 MB of memory, may use up to 1 GB, and need a little disk.
- A key of its own: `cast wallet new` (Foundry) or any Ethereum wallet tool.
  It needs no funds. Keep the private key on the host; we only need the
  address.
- The env file we send you, `dusk-validator.env`, with your bucket and its
  credentials.

```bash
umask 077
printf 'HYP_VALIDATOR_KEY=%s\n' '<private key, 0x...>' > validator-key.env
docker run -d --name dusk-validator --restart unless-stopped \
  --env-file dusk-validator.env --env-file validator-key.env \
  -v dusk-validator:/data -p 127.0.0.1:9091:9091 \
  ghcr.io/dusk-network/hyperlane-dusk-validator:<version>
```

Both come from the release: the image above, or the binary in
`hyperlane-dusk-validator-<version>-linux-x86_64.tar.gz`, run from its
directory:

```bash
set -a; . ../dusk-validator.env; . ../validator-key.env; set +a
CONFIG_FILES="$PWD/validator-dusk-operator.json" ./validator
```

It keeps its state under `/data`, as the image does; set `HYP_DB` and
`HYP_CHAINS_DUSK_EVENTCURSORDIR` to put it elsewhere. Both read Dusk through
the public node. If you run a Dusk node, such as a provisioner, use it:
`HYP_CHAINS_DUSK_CUSTOMRPCURLS=http://<your node>:8080/`. Validators that
all read one node sign whatever it tells them.

Send us the validator's address. On startup it writes its signed
announcement to your bucket and logs `Cannot announce validator without a
signer` until we announce it on Dusk for it. Then it logs `Validator has
announced signature storage location` and signs. `curl -s
127.0.0.1:9091/metrics | grep hyperlane_latest_checkpoint` shows the index
it has signed up to.

With enough other validators' signatures, a checkpoint lets the relayer
deliver a message to Sepolia, so keep the key secret. A stolen key alone,
below the threshold, can't get anything accepted. If it leaks, tell us and we
take it out of the set.

A validator that sees a reorg on Dusk writes `reorg_flag.json` to its bucket
and stops signing. While the flag is there, the relayer delivers nothing from
Dusk through an ISM that lists the validator, whatever the threshold. Tell us
if yours writes one, and before you stop for good.

## Add a validator

For the deployment's owner. A validator without a Dusk account can't
announce itself, so `announce` sends its signed announcement from
`DUSK_KEYS`'s account; the contract checks the validator's signature, not
who sends it.

1. Give the operator a bucket on Hetzner Object Storage in `nbg1`, behind
   `CHECKPOINT_S3_ENDPOINT`, in a project of its own (S3 keys cover every
   bucket in their project). Anyone must be able to get and list its
   objects, as with ours. Send `dusk-validator.env`:

   ```bash
   HYP_CHECKPOINTSYNCER_BUCKET=<bucket>
   HYP_CHECKPOINTSYNCER_REGION=nbg1
   HYP_CHECKPOINTSYNCER_FOLDER=<operator>
   AWS_ACCESS_KEY_ID=<key>
   AWS_SECRET_ACCESS_KEY=<secret>
   AWS_ENDPOINT_URL=https://nbg1.your-objectstorage.com
   ```

2. Once it runs, announce it, and record each validator already in the set
   the same way (for one that announced itself it only records):

   ```bash
   bash deploy/testnet.sh announce s3://<bucket>/nbg1/<operator> <its address>
   ```

3. Add it to `DUSK_ORIGIN_VALIDATORS`, set `DUSK_ORIGIN_THRESHOLD`, and run
   `bash deploy/testnet.sh set-validators`. It checks every validator in the
   new set: its newest announcement is the recorded location, it has no
   reorg flag, and its latest checkpoint is its own, for this deployment's
   tree, at most `VALIDATOR_CHECKPOINT_LAG` messages behind. It also checks
   that `DUSK_IGP_GAS_OVERHEAD` covers the threshold; raise it with
   `dusk-igp --apply` first if `fees` says so. Then `set-validators --apply`
   gets the new ISM from `EVM_ISM_FACTORY`, points every route's token at
   it, and records the switch once that is final. The relayer reads the ISM
   from the tokens, so its config doesn't change.

Add outside validators at a threshold ours still meet, 2 of 4, and raise it
to 3 of 4 once they have signed steadily for a week: only then does the
set need one of theirs. A validator that goes quiet stops nothing while the
others make the threshold, but one with a reorg flag stops every delivery.
To take one out, flagged or gone, remove it from `DUSK_ORIGIN_VALIDATORS`
and run `set-validators --apply`: it checks only the validators that stay.
