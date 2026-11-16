#!/usr/bin/env bash
set -euo pipefail
here=$(cd -- "$(dirname -- "$0")" && pwd)
target=${1:?Supply a new absolute build directory.}
[[ $target = /* && ! -e $target ]] || { echo 'Use a new absolute directory.' >&2; exit 1; }
emcc --version | head -1 | grep -F '3.1.69' >/dev/null || { echo 'Emscripten 3.1.69 is required.' >&2; exit 1; }
git clone https://github.com/kovacsv/occt-import-js.git "$target"
git -C "$target" checkout c2148e54b456b571238d35cac037d304053d64b2
git -C "$target" config submodule.occt.url https://github.com/Open-Cascade-SAS/OCCT.git
git -C "$target" submodule update --init --recursive --depth 1
[[ $(git -C "$target/occt" rev-parse HEAD) = d2abb6d844231cb8f29be6894440874a4700e4a5 ]]
cp -- "$here"/src/* "$target/occt-import-js/src/"
emcmake cmake -S "$target" -B "$target/build" -DCMAKE_BUILD_TYPE=Release
cmake --build "$target/build" --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-2}"
echo "Review output in $target/build/Release before replacing any shipped files."
