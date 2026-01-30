# wormhole-svm-test

Testing utilities for Solana programs integrating with Wormhole.

## Features

- **Guardian signing**: Create test guardians with configurable keys, sign VAA bodies
- **VAA construction**: Build and sign VAAs for testing
- **LiteSVM integration** (optional): Load Wormhole programs and set up guardian accounts
- **Signature helpers** (optional): Post/close guardian signatures with bracket pattern
- **Bundled fixtures** (optional): Pre-bundled mainnet program binaries for zero-setup testing

## Usage

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

Use `with_vaa` for the cleanest API. It automatically ensures your program actually
verifies VAAs:

1. **Negative test (on cloned SVM)**: Clones the SVM, posts mismatched signatures, and
   executes your transaction. If it succeeds, returns `VerificationBypass` error - your
   program isn't verifying! (Clone is discarded, no state changes persist.)
2. **Positive test (executed)**: Builds and sends your transaction with correct signatures.

```rust
use wormhole_svm_test::{with_vaa, TestVaa, emitter_address_from_20};

let vaa = TestVaa::new(1, emitter_address_from_20([0xAB; 20]), 42, payload);

// Closure receives (svm, sigs_pubkey, vaa_body) - body bytes for digest calculation
let result = with_vaa(
    &mut svm,
    &payer,
    &guardians,
    0, // guardian set index
    &vaa,
    |svm, sigs_pubkey, vaa_body| {
        let ix = build_my_verify_instruction(sigs_pubkey, vaa_body);
        let tx = Transaction::new_signed_with_payer(...);
        svm.send_transaction(tx).map_err(|e| format!("{:?}", e))
    },
)?;
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

### Manual Control

For full control over the signature lifecycle:

```rust
use wormhole_svm_test::{post_signatures, close_signatures, TestVaa};

let vaa = TestVaa::new(1, emitter, sequence, payload);
let vaa_body = vaa.body();  // Body bytes for digest calculation
let signatures = vaa.guardian_signatures(&guardians);

// Step 1: Post signatures
let posted = post_signatures(&mut svm, &payer, 0, &signatures)?;

// Step 2: Your verification logic using posted.pubkey and vaa_body
// ...

// Step 3: Close to reclaim rent
close_signatures(&mut svm, &payer, &posted.pubkey, &payer.pubkey())?;
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
