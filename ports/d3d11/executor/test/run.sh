#!/bin/sh
# Build VistaD3D11 and render_test with MinGW, linking vista_native built
# for Windows, and run it under Wine's Direct3D 11:
#
#   ports/d3d11/executor/test/run.sh OUT.png [render_test arguments...]
#
# Needs Wine (wine64), MinGW (x86_64-w64-mingw32-g++) and Rust's
# x86_64-pc-windows-gnu target (rustup target add x86_64-pc-windows-gnu).
# Wine draws with the host's OpenGL; with no GPU, run it in a virtual X
# display (Xvfb) on Mesa's llvmpipe, where the 960 x 600 default scene
# takes a minute or two.
#
# Wine's own Direct3D 11 (wined3d) translates shaders to GLSL, and on
# llvmpipe that turns a few terrain pixels near triangle edges black.
# DXVK, which games under Proton use, is a closer match to Windows: set
# DXVK_DIR to an unpacked DXVK release (its x64 directory is used) and
# install Mesa's Vulkan drivers (lavapipe) to run on it.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
executor=$(cd "$here/.." && pwd)
root=$(cd "$executor/../../.." && pwd)
work="$root/target/d3d11-executor"
mkdir -p "$work"

CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
  cargo build -q -p vista_native --release --target x86_64-pc-windows-gnu --manifest-path "$root/Cargo.toml"

x86_64-w64-mingw32-g++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -I"$executor" -I"$root/crates/vista_native/include" \
  "$executor/VistaD3D11.cpp" "$here/render_test.cpp" \
  "$root/target/x86_64-pc-windows-gnu/release/libvista_native.a" \
  -ld3d11 -ldxgi -lkernel32 -lntdll -luserenv -lws2_32 -ldbghelp -lbcrypt \
  -static -o "$work/render_test.exe"

wine=$(command -v wine64 || command -v wine || echo /usr/lib/wine/wine64)
export WINEPREFIX="${WINEPREFIX:-$work/prefix}" WINEDEBUG="${WINEDEBUG:--all}"

if [ -n "${DXVK_DIR:-}" ]; then
  cp "$DXVK_DIR/x64/d3d11.dll" "$DXVK_DIR/x64/dxgi.dll" "$work/"
  export WINEDLLOVERRIDES="d3d11,dxgi=n" DXVK_LOG_LEVEL="${DXVK_LOG_LEVEL:-none}"
else
  rm -f "$work/d3d11.dll" "$work/dxgi.dll"
fi

exec "$wine" "$work/render_test.exe" "$@"
