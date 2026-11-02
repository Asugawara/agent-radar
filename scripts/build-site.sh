#!/usr/bin/env bash
# Build the whole site: GitHub data (if GITHUB_TOKEN), favicons, table UI (WASM), HTML.
# Usage: scripts/build-site.sh <out-dir> [extra args for `radar build`]
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "usage: $0 <out-dir> [radar build args...]" >&2
  exit 2
fi
out="$1"
shift

cd "$(dirname "$0")/.."
export PATH="$PWD/target/tools/bin:$PATH"

enrich_args=()
if [ -n "${GITHUB_TOKEN:-}" ]; then
  cargo run -p radar-cli --release -- enrich --out target/enrich.json
fi
if [ -f target/enrich.json ]; then
  enrich_args=(--enrich target/enrich.json)
fi

# Favicons: fetched into target/favicons (cached, see --max-age-days); skip with
# RADAR_SKIP_FAVICONS=1. Failures to fetch never fail the build.
favicon_args=()
if [ "${RADAR_SKIP_FAVICONS:-}" != "1" ]; then
  cargo run -p radar-cli --release -- favicons --out target/favicons
fi
if [ -f target/favicons/manifest.json ]; then
  favicon_args=(--favicons target/favicons)
fi

# wasm-bindgen-cli must match the wasm-bindgen crate in Cargo.lock exactly.
# Install it under target/tools (not machine-wide) when missing or mismatched.
wbg_version=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | sed -n 's/version = "\(.*\)"/\1/p')
if [ "$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" != "$wbg_version" ]; then
  cargo install wasm-bindgen-cli --version "$wbg_version" --locked --root target/tools
fi

cargo build -p radar-web --target wasm32-unknown-unknown --profile wasm-release
rm -rf crates/radar-web/pkg
wasm-bindgen --target web --out-dir crates/radar-web/pkg \
  target/wasm32-unknown-unknown/wasm-release/radar_web.wasm
if command -v wasm-opt >/dev/null 2>&1; then
  # Features rustc enables by default for wasm32-unknown-unknown; older binaryen
  # (e.g. the apt package) rejects the module without them.
  if wasm-opt -Oz \
      --enable-bulk-memory --enable-mutable-globals --enable-nontrapping-float-to-int \
      --enable-reference-types --enable-sign-ext --enable-multivalue \
      crates/radar-web/pkg/radar_web_bg.wasm -o target/radar_web_bg.opt.wasm; then
    mv target/radar_web_bg.opt.wasm crates/radar-web/pkg/radar_web_bg.wasm
  else
    echo "warning: wasm-opt failed; using the unoptimized wasm" >&2
  fi
fi

cargo run -p radar-cli --release -- build --out "$out" ${enrich_args[@]+"${enrich_args[@]}"} ${favicon_args[@]+"${favicon_args[@]}"} "$@"
