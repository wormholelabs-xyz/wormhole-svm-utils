# wormhole-svm-test

Testing utilities for Solana programs integrating with Wormhole.

## Features

- **Guardian signing**: Create test guardians with configurable keys, sign VAA bodies
- **VAA construction**: Build and sign VAAs for testing
- **LiteSVM integration** (optional): Load Wormhole programs and set up guardian accounts
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

### Without Bundled Fixtures

If you prefer to manage your own binaries, use just the `litesvm` feature:

```toml
[dev-dependencies]
wormhole-svm-test = { version = "0.1", features = ["litesvm"] }
```

Then dump the programs from mainnet:

```bash
solana program dump --url https://api.mainnet-beta.solana.com \
    HDwcJBJXjL9FpJ7UBsYBtaDjsBUhuLCUYoz3zr8SWWaQ \
    fixtures/verify_vaa_shim.so

solana program dump --url https://api.mainnet-beta.solana.com \
    worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth \
    fixtures/core_bridge.so
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
