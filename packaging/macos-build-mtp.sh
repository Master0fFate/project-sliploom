set -euo pipefail

usb=libusb-1.0.30
mtp=libmtp-1.1.23
usb_sha=fea36f34f9156400209595e300840767ab1a385ede1dc7ee893015aea9c6dbaf
mtp_sha=74a2b6e8cb4a0304e95b995496ea3ac644c29371649b892b856e22f12a0bdeed
recipe=macos-build-mtp.sh

fail() {
  printf 'macOS MTP build: %s\n' "$*" >&2
  exit 1
}

mode=build
if [[ $# == 2 && $1 == --verify ]]; then
  mode=verify
  shift
fi
[[ $# == 1 ]] || fail 'usage: bash macos-build-mtp.sh [--verify] PREFIX'
[[ $(/usr/bin/uname -s) == Darwin && $(/usr/bin/uname -m) == arm64 ]] || fail 'requires native arm64 macOS'
prefix=$1
[[ "$prefix" =~ ^/[A-Za-z0-9_./-]+$ ]] || fail 'PREFIX must be absolute and contain no spaces or shell metacharacters'
parent=$(cd -- "$(dirname "$prefix")" && pwd -P)
[[ "$prefix" == "${parent%/}/${prefix##*/}" && ${prefix##*/} != . && ${prefix##*/} != .. && -n ${prefix##*/} ]] || fail 'PREFIX must be canonical'
sources="$prefix/sources"

verify_archive() {
  local file=$1 digest=$2
  [[ -f "$file" && ! -L "$file" ]] || fail "missing or unsafe archive: $file"
  printf '%s  %s\n' "$digest" "$file" | /usr/bin/shasum -a 256 -c -
}

runtime_hashes() (
  cd "$prefix/lib"
  /usr/bin/shasum -a 256 libusb-1.0.0.dylib libmtp.9.dylib
)

if [[ "$mode" == verify ]]; then
  [[ -d "$prefix" && ! -L "$prefix" && -d "$sources" && ! -L "$sources" ]] || fail 'missing or unsafe build prefix'
  files=("$sources/"*)
  [[ ${#files[@]} == 3 ]] || fail 'unexpected source payload'
  verify_archive "$sources/$usb.tar.bz2" "$usb_sha"
  verify_archive "$sources/$mtp.tar.gz" "$mtp_sha"
  [[ -f "$sources/$recipe" && ! -L "$sources/$recipe" ]] || fail 'missing or unsafe build recipe'
  /usr/bin/cmp "${BASH_SOURCE[0]}" "$sources/$recipe"
  actual=$(runtime_hashes)
  [[ -f "$prefix/runtime.sha256" && ! -L "$prefix/runtime.sha256" ]] || fail 'missing or unsafe build receipt'
  printf '%s\n' "$actual" | /usr/bin/cmp - "$prefix/runtime.sha256"
  exit 0
fi

pkg_config=$(command -v pkg-config) || fail 'pkg-config is required at build time'
[[ "$pkg_config" == /* && -x "$pkg_config" ]] || fail 'could not locate a pkg-config executable'
sdk=$(/usr/bin/xcrun --sdk macosx --show-sdk-path)
[[ -d "$sdk" ]] || fail 'Xcode or CLT macOS SDK not found'
/usr/bin/clang --version
mkdir -- "$prefix"
work="$prefix/build"
mkdir -- "$work" "$sources"
trap 'rm -rf -- "$work"' EXIT

fetch() {
  local url=$1 file=$2 digest=$3
  /usr/bin/curl -q --fail --location --retry 3 --proto '=https' --proto-redir '=https' --output "$file" "$url"
  verify_archive "$file" "$digest"
  /usr/bin/tar -xf "$file" -C "$work"
}

fetch "https://github.com/libusb/libusb/releases/download/v${usb#libusb-}/$usb.tar.bz2" "$sources/$usb.tar.bz2" "$usb_sha"
fetch "https://downloads.sourceforge.net/project/libmtp/libmtp/${mtp#libmtp-}/$mtp.tar.gz" "$sources/$mtp.tar.gz" "$mtp_sha"

build_env() {
  /usr/bin/env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME="$work" LC_ALL=C CONFIG_SITE=/dev/null \
    CC=/usr/bin/clang SDKROOT="$sdk" MACOSX_DEPLOYMENT_TARGET=11.0 \
    CFLAGS='-arch arm64 -O2' LDFLAGS='-arch arm64 -Wl,-headerpad_max_install_names' \
    PKG_CONFIG="$pkg_config" PKG_CONFIG_PATH= PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig" "$@"
}

configure=(--prefix="$prefix" --enable-shared --disable-static --disable-dependency-tracking)
(
  cd "$work/$usb"
  build_env ./configure "${configure[@]}" --disable-examples-build --disable-tests-build
  build_env /usr/bin/make install
)
(
  cd "$work/$mtp"
  [[ $(build_env "$pkg_config" --modversion libusb-1.0) == "${usb#libusb-}" ]] || fail 'pinned libusb pkg-config metadata unavailable'
  build_env ./configure "${configure[@]}" --disable-mtpz --disable-doxygen
  build_env /usr/bin/make -C src install
  build_env /usr/bin/make install-pkgconfigDATA
)
/usr/bin/install -m 644 "${BASH_SOURCE[0]}" "$sources/$recipe"
actual=$(runtime_hashes)
printf '%s\n' "$actual" > "$prefix/runtime.sha256"
