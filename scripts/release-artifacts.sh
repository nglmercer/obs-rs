#!/usr/bin/env bash
set -euo pipefail

artifact_dir="${1:-dist}"
mkdir -p "$artifact_dir"

# The pinned toolchain and release profile are the reproducibility inputs. A caller
# may set SOURCE_DATE_EPOCH in CI/release automation to make the build environment
# explicit without embedding a mutable timestamp in the repository.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
CARGO_INCREMENTAL=0 cargo build --workspace --release

for binary in obs-rs obs-rs-benchmark obs-rs-console obs-rs-web obs-rs-gui obs-rs-sandbox-source; do
    source="target/release/$binary"
    if [[ -x "$source" ]]; then
        cp "$source" "$artifact_dir/$binary"
    fi
done

(
    cd "$artifact_dir"
    sha256sum ./* 2>/dev/null | LC_ALL=C sort > SHA256SUMS
)

echo "Release artifacts written to $artifact_dir"
