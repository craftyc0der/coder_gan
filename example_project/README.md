# Unit Converter Library

A Rust library and CLI tool that converts between common units of measurement.
This project serves as the canonical demo for the **coder_gan** multi-agent
orchestration system.

## Problem Statement

Build a unit converter library with **two independent modules**:

### Module 1: Temperature (`src/temperature/`)

Implement the following pure functions in `src/temperature/mod.rs`:

| Function | Formula | Example |
|---|---|---|
| `celsius_to_fahrenheit(c: f64) -> f64` | F = C × 9/5 + 32 | 100.0 → 212.0 |
| `fahrenheit_to_celsius(f: f64) -> f64` | C = (F − 32) × 5/9 | 32.0 → 0.0 |
| `celsius_to_kelvin(c: f64) -> f64` | K = C + 273.15 | 0.0 → 273.15 |
| `kelvin_to_celsius(k: f64) -> f64` | C = K − 273.15 | 373.15 → 100.0 |

**Edge cases to handle:**
- Absolute zero: `kelvin_to_celsius(0.0)` → `-273.15`
- Negative temperatures are valid for Celsius and Fahrenheit
- The crossover point: `-40` is the same in both Celsius and Fahrenheit

### Module 2: Distance (`src/distance/`)

Implement the following pure functions in `src/distance/mod.rs`:

| Function | Formula | Example |
|---|---|---|
| `miles_to_km(mi: f64) -> f64` | km = mi × 1.60934 | 1.0 → 1.60934 |
| `km_to_miles(km: f64) -> f64` | mi = km / 1.60934 | 1.60934 → 1.0 |
| `meters_to_feet(m: f64) -> f64` | ft = m × 3.28084 | 1.0 → 3.28084 |
| `feet_to_meters(ft: f64) -> f64` | m = ft / 3.28084 | 3.28084 → 1.0 |

**Edge cases to handle:**
- Zero converts to zero for all functions
- Negative values should work (representing direction/debt, etc.)
- Round-trip accuracy: `km_to_miles(miles_to_km(x))` ≈ `x` within f64 precision

### CLI (`src/main.rs`)

The `main.rs` should demonstrate all 8 conversion functions with sample values
and print results to stdout. It is already provided as a skeleton — the agents
just need to implement the module functions so it compiles and runs.

## Architecture

```
src/
├── main.rs                  # CLI demo (provided, do not modify)
├── lib.rs                   # Module declarations (provided, do not modify)
├── temperature/
│   ├── mod.rs               # ← Coder Team 1 implements this
│   └── test/
│       └── mod.rs           # ← Tester Team 1 implements this
└── distance/
    ├── mod.rs               # ← Coder Team 2 implements this
    └── test/
        └── mod.rs           # ← Tester Team 2 implements this
```

## Development Rules

See [AGENTS.md](AGENTS.md) for the full development rules.

**Key rule:** All tests live in a `test/` subfolder next to the business logic,
never inline in the implementation file.

## Acceptance Criteria

The demo is **complete** when:

1. `cargo build` succeeds with no warnings
2. `cargo test` passes all tests (minimum 8 tests per module = 16 total)
3. `cargo run` prints correct sample conversions

## Reviewer TODO List

The reviewer agent (codex) should use this checklist to guide both teams:

### Phase 1 — Implementation (parallel)
- [ ] **Team Temp**: Implement all 4 functions in `src/temperature/mod.rs`
- [ ] **Team Dist**: Implement all 4 functions in `src/distance/mod.rs`
- [ ] Verify: `cargo build` succeeds

### Phase 2 — Testing (parallel)
- [ ] **Team Temp**: Write tests in `src/temperature/test/mod.rs` (≥8 tests)
- [ ] **Team Dist**: Write tests in `src/distance/test/mod.rs` (≥8 tests)
- [ ] Verify: `cargo test` passes all tests

### Phase 3 — Validation
- [ ] Verify: `cargo run` prints correct output
- [ ] Verify: No compiler warnings
- [ ] Verify: Round-trip conversions are accurate (within 1e-10 tolerance)
- [ ] Signal completion to orchestrator

## Quick Verification

Run these commands to confirm the demo is complete:

```bash
cargo build 2>&1 | tail -1        # should show "Finished"
cargo test  2>&1 | tail -1        # should show "test result: ok"
cargo run                          # should print 8 conversions
```
