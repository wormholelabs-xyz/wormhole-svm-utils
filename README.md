# wormhole-svm-utils

Testing, submission, and CLI utilities for Solana programs integrating with Wormhole.

This workspace contains three crates:

- **`wormhole-svm-test`** — LiteSVM testing helpers: guardian signing, VAA construction, environment setup, automatic verification and replay checks
- **`wormhole-svm-submit`** — Generic VAA submission via the executor-account-resolver protocol, with a `SolanaConnection` trait that abstracts over RPC and LiteSVM
- **`wormhole-svm-cli`** — CLI tool (`svm-vaa`) for submitting signed VAAs to any Solana program that implements the resolver protocol

## Workspace Structure

```
wormhole-svm-utils/
├── crates/
│   ├── wormhole-svm-test/       # Test utilities (guardians, VAA signing, LiteSVM helpers)
│   ├── wormhole-svm-submit/     # SolanaConnection trait + generic resolver/executor + RPC impl
│   └── wormhole-svm-cli/        # CLI binary: svm-vaa
├── programs/
│   ├── vaa-verifier-example/    # Example program: verify VAA via shim CPI
│   └── message-emitter-example/ # Example program: emit Wormhole message
```

## wormhole-svm-submit

Generic library for submitting signed VAAs to programs that implement the `resolve_execute_vaa_v1` instruction from [executor-account-resolver-svm](https://github.com/wormholelabs-xyz/executor-account-resolver-svm).

### SolanaConnection trait

The core abstraction that allows the same resolver/executor logic to work against both RPC and LiteSVM:

```rust
pub trait SolanaConnection {
    type Error: std::error::Error + Send + 'static;
    fn get_latest_blockhash(&self) -> Result<Hash, Self::Error>;
    fn simulate_return_data(&self, tx: &Transaction) -> Result<Option<Vec<u8>>, Self::Error>;
    fn send_and_confirm(&mut self, tx: &Transaction) -> Result<Signature, Self::Error>;
    fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, Self::Error>;
}
```

Built-in implementations:
- `impl SolanaConnection for RpcClient` — for CLI tools and production use
- `LiteSvmConnection` adapter in `wormhole-svm-test` — for tests

### RPC usage (broadcast_vaa)

For CLI tools and relayers, `broadcast_vaa` performs the complete flow: post signatures, resolve accounts, execute, close signatures.

```rust
use wormhole_svm_submit::broadcast_vaa;

let tx_sigs = broadcast_vaa(
    &mut rpc_client,
    &payer,
    &program_id,
    guardian_set_index,
    &vaa_body,
    &guardian_signatures,
    &core_bridge,
)?;
```

### Generic resolver

For custom integrations, use the resolver and executor directly with any `SolanaConnection`:

```rust
use wormhole_svm_submit::resolve::resolve_execute_vaa_v1;
use wormhole_svm_submit::execute::execute_instruction_groups;

let resolved = resolve_execute_vaa_v1(&conn, &program_id, &payer, &vaa_body, &guardian_set, 10)?;
let sigs = execute_instruction_groups(&mut conn, &payer, &resolved.instruction_groups, &sigs_pubkey, &guardian_set)?;
```

## wormhole-svm-cli (`svm-vaa`)

Solana CLI utilities: submit signed VAAs, derive PDAs, and inspect accounts.

### Install

```bash
cargo install --path crates/wormhole-svm-cli
```

### Global options

| Flag | Env var | Description |
|------|---------|-------------|
| `-u` / `--rpc-url` | `SOLANA_RPC_URL` | **Required.** Solana RPC endpoint or shorthand: `d`=devnet, `m`=mainnet, `l`=localhost, `t`=testnet |
| `--core-bridge` | `CORE_BRIDGE_PROGRAM_ID` | Wormhole Core Bridge program ID (auto-detected for mainnet/devnet URLs) |

### `submit` — Submit a signed VAA

```bash
# Submit a signed VAA (hex) to a program on devnet
svm-vaa -u d submit \
  --program-id <PROGRAM_ID> \
  --payer ~/.config/solana/id.json \
  <signed-vaa-hex>

# Read from file
svm-vaa -u d submit --program-id <PROGRAM_ID> --payer key.json @signed-vaa.hex

# Pipe from wsch (companion schema tool — https://github.com/wormholelabs-xyz/wormhole-schemas)
wsch build -s 'vaa<onboard>' --json payload.json | wsch sign --guardian-key $KEY \
  | svm-vaa -u d submit --program-id <PROGRAM_ID> --payer key.json
```

| Flag | Env var | Description |
|------|---------|-------------|
| `--program-id` | `PROGRAM_ID` | **Required.** Target program implementing the resolver protocol |
| `--payer` | `PAYER_KEYPAIR` | **Required.** Path to payer keypair file |

### `pda` — Derive a PDA

Seeds are strings by default, or hex with a `0x` prefix.

```bash
svm-vaa pda <PROGRAM_ID> foo bar baz
svm-vaa pda <PROGRAM_ID> 0xdeadbeef hello
```

### `account` — Dump account data

Prints account data as hex to stdout, metadata to stderr. The address can be a base58 pubkey or an inline PDA derivation (`<PROGRAM_ID>:seed1:seed2:...`).

```bash
# By address
svm-vaa -u m account <ADDRESS>

# By PDA derivation
svm-vaa -u m account <PROGRAM_ID>:seed1:0xdeadbeef
```

## wormhole-svm-test

### Features

- **Guardian signing**: Create test guardians with configurable keys, sign VAA bodies
- **VAA construction**: Build and sign VAAs for testing
- **LiteSVM integration** (optional): Load Wormhole programs and set up guardian accounts
- **Signature helpers** (optional): Post/close guardian signatures with bracket pattern
- **Bundled fixtures** (optional): Pre-bundled mainnet program binaries for zero-setup testing
- **Resolver** (optional): Account resolution via `wormhole-svm-submit` with LiteSVM adapter

### Usage

```rust
use wormhole_svm_test::{TestGuardian, TestGuardianSet, TestVaa};

// Create guardians
let guardians = TestGuardianSet::single(TestGuardian::default());

// Build and sign a VAA
let vaa = TestVaa::new(
    1,                    // emitter chain (Solana)
    [0xAB; 32],           // emitter address
    42,                   // sequence
    vec![1, 2, 3, 4],     // payload
);
let signed_vaa = vaa.sign(&guardians);
let signatures = vaa.guardian_signatures(&guardians);
```

### With LiteSVM (Recommended)

Use the `bundled-fixtures` feature for zero-setup testing:

```toml
[dev-dependencies]
wormhole-svm-test = { version = "0.1", features = ["bundled-fixtures"] }
```

```rust
use wormhole_svm_test::{
    TestGuardianSet, TestGuardian,
    setup_wormhole, WormholeProgramsConfig,
};
use litesvm::LiteSVM;

let mut svm = LiteSVM::new();
let guardians = TestGuardianSet::single(TestGuardian::default());

let wormhole = setup_wormhole(
    &mut svm,
    &guardians,
    0, // guardian set index
    WormholeProgramsConfig::default(),
)?;

// wormhole.guardian_set is the PDA address
```

### Verifying VAAs (Recommended)

Use `with_vaa` for the cleanest API. It automatically runs negative tests to ensure
your program validates VAAs correctly:

1. **Signature verification** — posts mismatched signatures; fails with `VerificationBypass` if accepted
2. **Emitter chain** — posts a VAA with wrong chain; fails with `EmitterChainBypass` if accepted
3. **Emitter address** — posts a VAA with wrong address; fails with `EmitterAddressBypass` if accepted
4. **Positive test** — executes with correct signatures (commits state)
5. **Replay protection** (if `NonReplayable`) — replays the same VAA; fails with `ReplayProtectionMissing` if accepted

All negative tests run on cloned SVMs that are discarded afterward — no state leaks.

Checks are controlled per-VAA via `VaaChecks` (all enabled by default):

```rust
use wormhole_svm_test::{with_vaa, TestVaa, VaaChecks, ReplayProtection, emitter_address_from_20};

let vaa = TestVaa::new(1, emitter_address_from_20([0xAB; 20]), 42, payload);

// All checks enabled by default
let result = with_vaa(&mut svm, &payer, &guardians, 0, &vaa, |svm, sigs_pubkey, vaa_body| {
    let ix = build_my_verify_instruction(sigs_pubkey, vaa_body);
    let tx = Transaction::new_signed_with_payer(...);
    svm.send_transaction(tx).map_err(|e| format!("{:?}", e))
})?;

// Disable specific checks when the program intentionally skips them
let mut vaa = TestVaa::new(1, [0xAB; 32], 42, payload);
vaa.checks.emitter_address = false;          // e.g. initialize accepts any emitter
vaa.checks.replay = ReplayProtection::Replayable; // e.g. idempotent operations
```

### Full End-to-End: broadcast_vaa (Recommended)

`broadcast_vaa` is the test-crate counterpart to [`wormhole_svm_submit::broadcast_vaa`](#rpc-usage-broadcast_vaa). It runs the complete resolve → post-signatures → execute → close-signatures flow, wrapped in `with_vaa` so you get all the safety checks automatically.

```toml
[dev-dependencies]
wormhole-svm-test = { version = "0.1", features = ["bundled-fixtures", "resolver"] }
```

```rust
use wormhole_svm_test::{broadcast_vaa, TestVaa, TestGuardianSet, TestGuardian};

let guardians = TestGuardianSet::single(TestGuardian::default());
let vaa = TestVaa::new(1, [0xAB; 32], 42, payload);

let tx_sigs = broadcast_vaa(
    &mut svm,
    &payer,
    &program_id,
    &guardians,
    0, // guardian set index
    &vaa,
)?;
```

### Resolver integration

Enable the `resolver` feature to use the account resolver with LiteSVM directly:

```toml
[dev-dependencies]
wormhole-svm-test = { version = "0.1", features = ["bundled-fixtures", "resolver"] }
```

```rust
use wormhole_svm_test::resolve_execute_vaa_v1;

let result = resolve_execute_vaa_v1(
    &mut svm,
    &program_id,
    &payer,
    &vaa_body,
    &wormhole.guardian_set,
    10, // max iterations
)?;

// result.instruction_groups contains the resolved instructions
// result.iterations shows how many rounds it took
```

### Lower-Level: with_posted_signatures

If you need more control over VAA construction and signing:

```rust
use wormhole_svm_test::{with_posted_signatures, TestVaa, TestGuardianSet};

let vaa = TestVaa::new(1, emitter, sequence, payload);
let vaa_body = vaa.body();  // Body bytes for digest calculation
let signatures = vaa.guardian_signatures(&guardians);

// Bracket pattern: post signatures, run closure, close signatures
with_posted_signatures(
    &mut svm,
    &payer,
    0, // guardian set index
    &signatures,
    |svm, sigs_pubkey| {
        // Your code that uses verify_hash CPI with vaa_body goes here
        Ok(())
    },
)?;
```

### Without Bundled Fixtures

If you prefer to manage your own binaries, use just the `litesvm` feature:

```toml
[dev-dependencies]
wormhole-svm-test = { version = "0.1", features = ["litesvm"] }
```

Then dump the programs from mainnet:

```bash
solana program dump --url https://api.mainnet-beta.solana.com \
    EFaNWErqAtVWufdNb7yofSHHfWFos843DFpu4JBw24at \
    fixtures/verify_vaa_shim.so

solana program dump --url https://api.mainnet-beta.solana.com \
    worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth \
    fixtures/core_bridge.so

solana program dump --url https://api.mainnet-beta.solana.com \
    EtZMZM22ViKMo4r5y4Anovs3wKQ2owUmDpjygnMMcdEX \
    fixtures/post_message_shim.so
```

Or set `WORMHOLE_FIXTURES_DIR` to point to existing binaries.

## Multi-Guardian Testing

```rust
// Generate deterministic guardians
let guardians = TestGuardianSet::generate(13, 12345);

// Sign with all
let signed = vaa.sign(&guardians);

// Sign with quorum subset
let signed = vaa.sign_with(&guardians, &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
```

## License

Apache-2.0
