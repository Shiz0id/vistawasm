#!/bin/sh
# Compile every translated shader with Microsoft's own HLSL compiler, on
# Linux, the way fxc would on Windows.
#
#   ports/d3d11/tools/check-hlsl.sh [OUT_DIR]
#
# Writes the compiled shaders to OUT_DIR, by default ports/d3d11/cso (the
# committed copy), as <module>/<entry>.cso beside the manifest's
# hlsl/<module>/<entry>.hlsl. Compiling is deterministic, so an unchanged
# shader writes the same bytes. Expect about 12 minutes: the texture bake's
# two shaders take 3 minutes each.
#
# Needs Wine (wine64) and MinGW (x86_64-w64-mingw32-gcc). Microsoft's
# D3DCompiler_43.dll comes from the Microsoft.DXSDK.D3DX NuGet package,
# under Microsoft's licence; it is downloaded to target/, never committed.
#
# The exit status is the number of shaders that failed. Warnings are
# printed but do not fail the check: the translation keeps WGSL's own
# arithmetic, which fxc warns about (integer division, pow of a negative).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
port=$(cd "$here/.." && pwd)
root=$(cd "$port/../.." && pwd)
work="$root/target/check-hlsl"
package_version=9.29.952.8
mkdir -p "$work"

wine=$(command -v wine64 || command -v wine || echo /usr/lib/wine/wine64)

if [ ! -x "$wine" ]; then
  echo "Wine is not installed: apt-get install wine64" >&2
  exit 2
fi

if [ ! -f "$work/D3DCompiler_43.dll" ]; then
  curl -sSfL -o "$work/d3dx.nupkg" \
    "https://api.nuget.org/v3-flatcontainer/microsoft.dxsdk.d3dx/$package_version/microsoft.dxsdk.d3dx.$package_version.nupkg"
  python3 -I -c "import sys, zipfile; z = zipfile.ZipFile(sys.argv[1]); open(sys.argv[2], 'wb').write(z.read('build/native/release/bin/x64/D3DCompiler_43.dll'))" \
    "$work/d3dx.nupkg" "$work/D3DCompiler_43.dll"
fi

x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -Werror -o "$work/hlslc.exe" "$here/hlslc.c"

# Every file the manifest lists, with its entry point and profile.
python3 -I -c "
import json, sys
manifest = json.load(open(sys.argv[1]))
for f in manifest['files']:
  print('hlsl/%s %s %s' % (f['file'], f['entryPoint'], f['profile']))
" "$port/hlsl/manifest.json" > "$work/list.txt"

out="$port/cso"

if [ $# -ge 1 ]; then
  out=$1
fi

mkdir -p "$out"
out=$(cd "$out" && pwd)

cd "$port"
export WINEPREFIX="$work/prefix" WINEDEBUG=-all WINEDLLOVERRIDES="d3dcompiler_43=n"
# Without the flag the tool does not treat warnings as errors.
"$wine" "$work/hlslc.exe" "$work/D3DCompiler_43.dll" "$work/list.txt" "$out"
