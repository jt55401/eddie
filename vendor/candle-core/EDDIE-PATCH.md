# Vendored candle-core 0.11.0

Unmodified crates.io `candle-core` 0.11.0 source (tests, benches and examples
removed) with one change in `src/cpu/mod.rs`: the `vec_add_f16`, `vec_add_bf16`,
`vec_scalar_add_f16` and `vec_scalar_add_bf16` SIMD fast paths are no longer selected for
`target_feature = "simd128"`, because `cpu/simd128.rs` defines only
`CurrentCpu` (f32) and the crate fails to compile for
`wasm32-unknown-unknown` with `-C target-feature=+simd128`.

Remove `[patch.crates-io] candle-core` from the root `Cargo.toml` and delete
this directory once a candle release includes the fix.
