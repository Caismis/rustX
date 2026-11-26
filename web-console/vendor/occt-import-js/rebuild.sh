#!/usr/bin/env bash
set -euo pipefail
here=$(cd -- "$(dirname -- "$0")" && pwd)
target=${1:?Supply a new absolute build directory.}
optimization=${2:-O3}
lto=${3:-off}
readonly importer_commit=c2148e54b456b571238d35cac037d304053d64b2
readonly occt_commit=d2abb6d844231cb8f29be6894440874a4700e4a5
[[ $target = /* && ! -e $target ]] || { echo 'Use a new absolute directory.' >&2; exit 1; }
[[ $optimization =~ ^(Oz|O2|O3)$ && $lto =~ ^(on|off)$ ]] || { echo 'Expected Oz/O2/O3 and on/off LTO.' >&2; exit 1; }
emcc --version | head -1 | grep -F '3.1.69' >/dev/null || { echo 'Emscripten 3.1.69 is required.' >&2; exit 1; }
# A local immutable checkout avoids repeated network fetches for paired builds.
if [[ -n ${4:-} ]]; then
    [[ $(git -C "$4" rev-parse HEAD) = "$importer_commit" ]]
    [[ $(git -C "$4/occt" rev-parse HEAD) = "$occt_commit" ]]
    git -C "$4" diff --quiet "$importer_commit" --
    git -C "$4/occt" diff --quiet "$occt_commit" --
    mkdir -p "$target"
    git -C "$4" archive "$importer_commit" CMakeLists.txt occt-import-js | tar -x -C "$target"
    mkdir -- "$target/occt"
    git -C "$4/occt" archive "$occt_commit" | tar -x -C "$target/occt"
else
    git clone https://github.com/kovacsv/occt-import-js.git "$target"
    git -C "$target" checkout "$importer_commit"
    git -C "$target" config submodule.occt.url https://github.com/Open-Cascade-SAS/OCCT.git
    git -C "$target" submodule update --init --recursive --depth 1
    [[ $(git -C "$target/occt" rev-parse HEAD) = "$occt_commit" ]]
fi
GIT_CEILING_DIRECTORIES="$target" git -C "$target/occt" apply "$here/step-bulk-lexer.patch"
cp -- "$here"/src/* "$target/occt-import-js/src/"
python3 "$here/configure-build.py" "$target" "$optimization" "$lto"
# Set Release explicitly to avoid CMake's implicit -O3 competing with the choice.
emcmake cmake -G Ninja -S "$target" -B "$target/build" -DCMAKE_BUILD_TYPE=Release -DCMAKE_C_FLAGS_RELEASE=-DNDEBUG -DCMAKE_CXX_FLAGS_RELEASE=-DNDEBUG -DCMAKE_EXPORT_COMPILE_COMMANDS=ON
cmake --build "$target/build" --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-8}"
# Keep actual generated flags with the artifact, including the complete link command.
ninja -C "$target/build" -t commands > "$target/effective-commands.txt"
python3 "$here/audit-build.py" "$target"
echo "Review output in $target/build/Release before replacing any shipped files."
