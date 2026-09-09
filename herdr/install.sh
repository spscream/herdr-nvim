#!/usr/bin/env bash
# herdr plugin build hook: fetch the prebuilt binary for this platform, or
# build from source as a fallback (reviewr pattern).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p bin
version=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -1)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
  *) target="" ;;
esac
url="https://github.com/ChmaraX/herdr-nvim/releases/download/v${version}/herdr-nvim-${target}"

# Download the release build for this platform, and prove that it starts
# before accepting it. A binary that downloads is not a binary that runs: a
# release linked against a newer glibc than the host's fetches fine and then
# dies in the loader. Since curl succeeded, the source fallback below would
# never fire, and the install would report success with a file that cannot
# execute.
fetch_prebuilt() {
  [ -n "$target" ] || return 1
  curl -fsSL "$url" -o bin/herdr-nvim.tmp || return 1
  chmod +x bin/herdr-nvim.tmp
  local rc=0
  # An unknown subcommand prints usage and exits 2. The probe wants no side
  # effect -- only proof that the process starts.
  bin/herdr-nvim.tmp --version >/dev/null 2>&1 || rc=$?
  # 126 = file is not executable. 127 = loader or interpreter missing. Every
  # other code means the process itself ran, which is all this asks.
  if [ "$rc" -ge 126 ]; then
    echo "herdr-nvim: prebuilt binary does not run here (exit $rc)" >&2
    rm -f bin/herdr-nvim.tmp
    return 1
  fi
  mv bin/herdr-nvim.tmp bin/herdr-nvim
}

if fetch_prebuilt; then
  :
elif command -v cargo >/dev/null; then
  echo "herdr-nvim: no usable prebuilt binary; building from source" >&2
  cargo build --release
  cp target/release/herdr-nvim bin/herdr-nvim
else
  echo "herdr-nvim: no prebuilt binary for ${target:-$(uname -s)-$(uname -m)} and no cargo" >&2
  exit 1
fi
