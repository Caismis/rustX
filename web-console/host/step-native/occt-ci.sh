#!/usr/bin/env bash
set -euo pipefail

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
  echo 'The CI OCCT dependency is built for Linux x86_64.' >&2
  exit 1
fi

recipe=$(realpath "$0")
patch=$(realpath "$(dirname "$recipe")/../../vendor/occt-import-js/step-bulk-lexer.patch")
revision=d2abb6d844231cb8f29be6894440874a4700e4a5

inputs() {
  printf 'OCCT %s\n' "$revision"
  printf 'Patch %s\n' "$(sha256sum "$patch" | cut -d ' ' -f 1)"
  printf 'Recipe %s\n' "$(sha256sum "$recipe" | cut -d ' ' -f 1)"
  printf 'C++ %s %s\n' "$(c++ -dumpmachine)" "$(c++ -dumpfullversion -dumpversion)"
  printf 'CMake %s\n' "$(cmake --version | sed -n '1p')"
  dpkg-query --show --showformat='${Package} ${Version}\n' libc6-dev libx11-dev rapidjson-dev
  grep -E '^(ID|VERSION_ID)=' /etc/os-release
}

manifest() {
  (
    cd "$1"
    LC_ALL=C find include lib -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum
    while IFS= read -r -d '' link; do
      # OCCT installs versioned libraries as relative symlinks.
      target=$(readlink "$link")
      [[ "$target" != */* && -f "$link" ]]
      printf 'link %s %s\n' "$link" "$target"
    done < <(LC_ALL=C find include lib -type l -print0 | LC_ALL=C sort -z)
  )
}

case "${1:-}" in
  key)
    inputs | sha256sum | cut -d ' ' -f 1
    ;;
  build)
    prefix=$2
    [[ "$prefix" == /* && ! -e "$prefix" ]]
    scratch=$(mktemp -d)
    trap 'rm -rf -- "$scratch"' EXIT
    git init --quiet "$scratch/source"
    git -C "$scratch/source" fetch --quiet --depth=1 https://github.com/Open-Cascade-SAS/OCCT.git "$revision"
    [[ $(git -C "$scratch/source" rev-parse FETCH_HEAD) == "$revision" ]]
    git -C "$scratch/source" checkout --quiet --detach FETCH_HEAD
    git -C "$scratch/source" apply "$patch"
    cmake -S "$scratch/source" -B "$scratch/build" -G Ninja -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_POLICY_VERSION_MINIMUM=3.5 -DINSTALL_DIR="$prefix" \
      -DINSTALL_DIR_LIB=lib -DINSTALL_DIR_INCLUDE=include/opencascade \
      '-DBUILD_ADDITIONAL_TOOLKITS=TKXDESTEP TKRWMesh' -DUSE_RAPIDJSON=ON \
      -DUSE_FREETYPE=OFF -DUSE_TBB=OFF -DUSE_OPENGL=OFF \
      -DBUILD_MODULE_FoundationClasses=OFF -DBUILD_MODULE_ModelingData=OFF \
      -DBUILD_MODULE_ModelingAlgorithms=OFF -DBUILD_MODULE_Visualization=OFF \
      -DBUILD_MODULE_ApplicationFramework=OFF -DBUILD_MODULE_DataExchange=OFF -DBUILD_MODULE_Draw=OFF
    cmake --build "$scratch/build" --parallel "$(nproc)"
    cmake --install "$scratch/build" --prefix "$prefix"
    inputs > "$prefix/rustx-occt-inputs.txt"
    manifest "$prefix" > "$prefix/rustx-occt-sha256.txt"
    ;;
  verify)
    prefix=$2
    test -f "$prefix/include/opencascade/Standard_Version.hxx"
    test -f "$prefix/lib/libTKXDESTEP.so"
    cmp "$prefix/rustx-occt-inputs.txt" <(inputs)
    cmp "$prefix/rustx-occt-sha256.txt" <(manifest "$prefix")
    cat "$prefix/rustx-occt-inputs.txt"
    ;;
  *)
    echo 'Expected key, build <prefix> or verify <prefix>.' >&2
    exit 1
    ;;
esac
