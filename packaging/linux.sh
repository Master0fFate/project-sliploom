set -euo pipefail
export LC_ALL=C

version=$1
package=$2
binary=apps/desktop/target/release/sliploom
notice=apps/desktop/assets/Roboto-OFL.txt
architecture=$(dpkg --print-architecture)
test "$architecture" = amd64
readelf -h "$binary" | grep -E 'Machine:[[:space:]]+Advanced Micro Devices X86-64'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage="$work/package"
mkdir -p "$(dirname "$package")" "$work/debian" "$stage/DEBIAN" "$stage/usr/bin" \
  "$stage/usr/share/applications" "$stage/usr/share/doc/sliploom"
install -m 755 "$binary" "$stage/usr/bin/sliploom"
install -m 644 packaging/sliploom.desktop "$stage/usr/share/applications/sliploom.desktop"
install -m 644 "$notice" "$stage/usr/share/doc/sliploom/Roboto-OFL.txt"

cat > "$work/debian/control" <<'EOF'
Source: sliploom

Package: sliploom
Architecture: any
EOF
shlibs=$(cd "$work" && dpkg-shlibdeps -O -e"$stage/usr/bin/sliploom")
[[ "$shlibs" == shlibs:Depends=?* ]]
depends=${shlibs#shlibs:Depends=}
if [[ ! ", $depends," =~ ,[[:space:]]*libmtp9t64([[:space:]]|,) ]]; then
  printf 'dpkg-shlibdeps did not detect the Ubuntu 24.04 MTP library: %s\n' "$depends" >&2
  exit 1
fi
test "$(dpkg-query -W -f='${Status}' libmtp9t64)" = 'install ok installed'
for dependency in libmtp-runtime libfontconfig1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libwayland-cursor0 \
  libx11-6 libx11-xcb1 libxcursor1 libxi6 libxrandr2 libegl1 libgl1 libvulkan1 xdg-utils; do
  if [[ ! ", $depends," =~ ,\ ${dependency}([[:space:]]|,) ]]; then
    depends+=", $dependency"
  fi
done

cat > "$stage/DEBIAN/control" <<EOF
Package: sliploom
Version: $version
Section: net
Priority: optional
Architecture: $architecture
Maintainer: Sliploom contributors
Depends: $depends
Recommends: xdg-desktop-portal
Description: Local file transfer between desktop and Android
 Sliploom sends and receives files over a local network.
EOF

dpkg-deb --root-owner-group --build "$stage" "$package"
test -s "$package"
test "$(dpkg-deb --field "$package" Package)" = sliploom
test "$(dpkg-deb --field "$package" Version)" = "$version"
test "$(dpkg-deb --field "$package" Architecture)" = "$architecture"
test "$(dpkg-deb --field "$package" Depends)" = "$depends"
dpkg-deb --extract "$package" "$work/verify"
test -x "$work/verify/usr/bin/sliploom"
cmp "$binary" "$work/verify/usr/bin/sliploom"
cmp "$notice" "$work/verify/usr/share/doc/sliploom/Roboto-OFL.txt"
desktop-file-validate "$work/verify/usr/share/applications/sliploom.desktop"
test "$("$work/verify/usr/bin/sliploom" --version)" = "Sliploom $version"
linked=$(ldd "$work/verify/usr/bin/sliploom")
if [[ "$linked" == *'not found'* || ! "$linked" =~ libmtp\.so\.9[[:space:]]+\=\>[[:space:]]+/ ]]; then
  printf 'Could not resolve the packaged binary’s shared libraries:\n%s\n' "$linked" >&2
  exit 1
fi

cd "$(dirname "$package")"
filename=$(basename "$package")
apt-get --simulate install --no-install-recommends "$PWD/$filename"
sha256sum -- "$filename" > "$filename.sha256"
