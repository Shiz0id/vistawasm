#!/bin/sh
# Build SelotapeVista against vista_native and run its test.
#
#   run.sh [SELOTAPE_INCLUDE] [--large]
#
# SELOTAPE_INCLUDE is the directory holding selotape/SelotapeTerrain.h.
# Without it the test builds against test/standin/selotape/SelotapeTerrain.h,
# which declares only what SelotapeVista uses. Selotape's other headers are
# stood in for by test/stubs, and its PackLayerWeights by
# test/stubs/selotape_terrain_stub.cpp. --large adds a 4 km map at 1 m.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
out="$root/target/selotape-vista-test"
mkdir -p "$out/selotape"

include="$here/standin"

if [ $# -ge 1 ] && [ "$1" != "--large" ]; then
  include=$1
  shift
fi

cargo build --manifest-path "$root/Cargo.toml" -p vista_native --release

# Only SelotapeTerrain.h is taken from Selotape, so a stub never shadows a
# real header the build would otherwise find.
cp "$include/selotape/SelotapeTerrain.h" "$out/selotape/"

libs=$(cargo rustc --manifest-path "$root/Cargo.toml" -p vista_native --release \
  --crate-type staticlib -- --print native-static-libs 2>&1 \
  | sed -n 's/.*native-static-libs: //p' | tail -n 1)

${CXX:-c++} -std=c++17 -O2 -Wall -Wextra -Werror -pthread \
  -I "$out" -I "$here/stubs" -I "$here/.." -I "$root/crates/vista_native/include" \
  "$here/selotape_vista_test.cpp" "$here/../SelotapeVista.cpp" "$here/stubs/selotape_terrain_stub.cpp" \
  "$root/target/release/libvista_native.a" $libs \
  -o "$out/selotape_vista_test"

"$out/selotape_vista_test" "$@"
