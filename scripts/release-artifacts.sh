#!/usr/bin/env bash
set -euo pipefail

artifact_dir="${1:-dist}"
target="x86_64-unknown-linux-gnu"
: "${OBS_RS_RELEASE_SIGNING_KEY:?set OBS_RS_RELEASE_SIGNING_KEY to an Ed25519 private-key PEM}"
mkdir -p "$artifact_dir"

# The pinned toolchain and release profile are the reproducibility inputs. A caller
# may set SOURCE_DATE_EPOCH in CI/release automation to make the build environment
# explicit without embedding a mutable timestamp in the repository.
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-0}"
if [[ "$(rustc -vV | sed -n 's/^host: //p')" != "$target" ]]; then
    echo "release signing is gated to $target" >&2
    exit 1
fi

CARGO_INCREMENTAL=0 cargo build --workspace --release --locked

stage="$(mktemp -d "${TMPDIR:-/tmp}/obs-rs-release.XXXXXX")"
trap 'rm -rf -- "$stage"' EXIT
package="$stage/obs-rs"
mkdir -p "$package/bin" "$package/share/applications" "$package/share/licenses"

for binary in obs-rs obs-rs-benchmark obs-rs-console obs-rs-web obs-rs-gui obs-rs-sandbox-source; do
    source="target/release/$binary"
    if [[ -x "$source" ]]; then
        install -m 0755 "$source" "$package/bin/$binary"
    fi
done

install -m 0644 packaging/obs-rs.desktop "$package/share/applications/obs-rs.desktop"
install -m 0644 packaging/gstreamer-plugin-allowlist.txt \
    "$package/share/licenses/gstreamer-plugin-allowlist.txt"

cargo tree --workspace --edges normal --prefix none | LC_ALL=C sort -u \
    > "$package/share/licenses/RUST-DEPENDENCIES.txt"

{
    echo "SPDXVersion: SPDX-2.3"
    echo "DataLicense: CC0-1.0"
    echo "SPDXID: SPDXRef-DOCUMENT"
    echo "DocumentName: obs-rs-${target}"
    echo "DocumentNamespace: https://obs-rs.invalid/spdx/${target}/${SOURCE_DATE_EPOCH}"
    echo "Creator: Tool: cargo-tree"
    echo "Created: 1970-01-01T00:00:00Z"
    while IFS= read -r dependency; do
        name="${dependency%% v*}"
        version="${dependency#* v}"
        echo
        echo "PackageName: $name"
        echo "SPDXID: SPDXRef-Package-$(printf '%s' "$name-$version" | tr -cd '[:alnum:].-')"
        echo "PackageVersion: $version"
        echo "PackageDownloadLocation: NOASSERTION"
        echo "PackageLicenseConcluded: NOASSERTION"
    done < "$package/share/licenses/RUST-DEPENDENCIES.txt"
} > "$package/SBOM.spdx"

{
    echo "format=OBSRRELEASE1"
    echo "target=$target"
    echo "source_date_epoch=$SOURCE_DATE_EPOCH"
    echo "rustc=$(rustc --version)"
    echo "cargo=$(cargo --version)"
    echo "gstreamer_allowlist=share/licenses/gstreamer-plugin-allowlist.txt"
    echo "sbom=SBOM.spdx"
} > "$package/RELEASE-MANIFEST"

(
    cd "$package"
    find . -type f ! -name SHA256SUMS -print0 \
        | LC_ALL=C sort -z \
        | xargs -0 sha256sum > SHA256SUMS
)

archive="$artifact_dir/obs-rs-${target}.tar.gz"
tar --sort=name --mtime="@${SOURCE_DATE_EPOCH}" --owner=0 --group=0 --numeric-owner \
    -C "$stage" -cf - obs-rs | gzip -n -9 > "$archive"
sha256sum "$archive" > "$archive.sha256"
openssl pkeyutl -sign -rawin -inkey "$OBS_RS_RELEASE_SIGNING_KEY" \
    -in "$archive.sha256" -out "$archive.sha256.sig"

(
    cd "$artifact_dir"
    sha256sum ./* 2>/dev/null | LC_ALL=C sort > SHA256SUMS
)

echo "Signed reproducible release archive written to $archive"
