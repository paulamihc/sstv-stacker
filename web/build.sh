#!/bin/sh
# Builds the web page into web/dist: the page's files and the WebAssembly.
# Needs the wasm32-unknown-unknown target (rustup target add
# wasm32-unknown-unknown). Netlify publishes web/dist as it is.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/.."
# Paths on the building computer (in error messages) are left out of the
# published file.
# (Passed one per line, so a folder name with spaces stays whole.)
CARGO_ENCODED_RUSTFLAGS=$(printf '%s\037%s' "--remap-path-prefix=$(pwd)=sstv-stacker" "--remap-path-prefix=$HOME=~")
export CARGO_ENCODED_RUSTFLAGS
cargo build -p sstv-stacker-web --target wasm32-unknown-unknown --profile web
target=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
mkdir -p "$here/dist"
cp "$here"/src/index.html "$here"/src/style.css "$here"/src/app.js "$here"/src/worker.js "$here"/src/icon.svg "$here/dist/"
cp "$target/wasm32-unknown-unknown/web/sstv_stacker_web.wasm" "$here/dist/"
echo "Built $here/dist ($(wc -c < "$here/dist/sstv_stacker_web.wasm" | tr -d ' ') bytes of WebAssembly)"
