#!/bin/sh
# Build vista_native as a static library and run the C++ smoke test
# against it. Needs a C++17 compiler as `c++` (or set CXX).
set -eu

root=$(cd "$(dirname "$0")/../../.." && pwd)
out="$root/target/vista-native-smoke"
mkdir -p "$out"

cargo build --manifest-path "$root/Cargo.toml" -p vista_native --release

# The system libraries a Rust static library needs on this platform.
libs=$(cargo rustc --manifest-path "$root/Cargo.toml" -p vista_native --release \
  --crate-type staticlib -- --print native-static-libs 2>&1 \
  | sed -n 's/.*native-static-libs: //p' | tail -n 1)

${CXX:-c++} -std=c++17 -O2 -Wall -Wextra -Werror \
  -I "$root/crates/vista_native/include" \
  "$root/crates/vista_native/examples/smoke.cpp" \
  "$root/target/release/libvista_native.a" $libs \
  -o "$out/smoke"

"$out/smoke" "$out/vista-height.pgm"
