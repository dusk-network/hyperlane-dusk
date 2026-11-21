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

Both read Dusk through the public node, `testnet.nodes.dusk.network`. If you
run a Dusk node, such as a provisioner, point the validator at it with
`HYP_CHAINS_DUSK_CUSTOMRPCURLS=http://<your node>:8080/`. Validators that
all read one node all sign whatever it tells them: one of their own each
is what makes them independent.

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

A validator that sees a reorg on Dusk writes `reorg_flag.json` to its bucket
and stops signing. While the flag is there, the relayer delivers nothing
from Dusk through an ISM that lists the validator, whatever the threshold.
Tell us if yours writes one.

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

   It refuses unless every validator in the new set has announced the
   location recorded for it as its newest, has no reorg flag, and has signed
   one of the last `DUSK_VALIDATOR_CHECKPOINT_LAG` messages, for this
   deployment's tree: message ID and root, checked through the Dusk node.
   Then switch:

   ```bash
   bash deploy/testnet.sh set-validators --apply
   ```

   First it dispatches a probe from Dusk's TestRecipient (17.06 DUSK at the
   default prices, from `DUSK_KEYS`) and waits, up to
   `VALIDATOR_PROBE_SECONDS`, for every validator in the new set to sign it:
   one that signed the last message and then stopped passes the count, not
   the probe. The probe goes to `VALIDATOR_PROBE_RECIPIENT`, the zero address
   unless set, so the relayer can't deliver it and retries it with backoff
   without paying. Sepolia's MessageIdMultisig ISMs can't be changed, so it
   then deploys one for the new set through `EVM_ISM_FACTORY` and points
   every route's token at it, and records the switch once those updates
   are final. The relayer reads the ISM from the token, so it needs no new
   config.

A lower threshold keeps transfers going while a new validator settles in
only as long as it signs: any validator in the set can stop every delivery
from Dusk with a reorg flag (above), and the relayer honours it from every
validator the ISM lists, whatever the threshold. This holds for the agents
we run (`6f91b2a`) and for fork main. The threshold has to exceed the
validators we run before outside validators add security: 3 of 4 with two
of ours and two outside.

To take a validator out, flagged or gone, remove it from
`DUSK_ORIGIN_VALIDATORS`, lower `DUSK_ORIGIN_THRESHOLD` if needed, and run
`set-validators --apply`. It checks and probes only the validators that
stay, so a flagged one doesn't block its own removal. Once the tokens
verify with the new ISM, the relayer stops reading the old one's
validators, and deliveries resume.

`EVM_ORIGIN_VALIDATORS` and `EVM_ORIGIN_THRESHOLD` change the same way:
`set-validators` sets them on the Dusk ISMs the routes use. EVM-origin
validators announce themselves on Sepolia, as any Hyperlane validator does,
and their checkpoints may be `EVM_VALIDATOR_CHECKPOINT_LAG` messages behind
Sepolia's shared tree. A higher threshold needs a higher
`DUSK_RELAYER_GAS_LIMIT`, and the running relayer must have it before the
threshold goes up: regenerate the agent configs with it (`agent-configs`),
restart the relayer, then switch. `set-validators` reads the gas limit from
`RELAYER_CONFIG`, by default the `relayer.json` agent-configs wrote, and
refuses without one unless `RELAYER_GAS_LIMIT_UNCHECKED=1`: point it at the
config the relayer runs.
