# OBS-RS fuzzing

Install `cargo-fuzz`, then run a target such as:

```sh
cargo fuzz run project_config -- -max_total_time=60
```

Seed inputs live under `corpus/`. Keep minimized crash artifacts in the
matching corpus directory so every discovered regression remains covered.
