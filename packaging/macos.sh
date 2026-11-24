set -euo pipefail

version=$1
package=$2
binary=apps/desktop/target/release/sliploom
notice=apps/desktop/assets/Roboto-OFL.txt
stage=$(mktemp -d "${TMPDIR:-/tmp}/sliploom.XXXXXXXX")
mount="$stage/mount"
attached=no

cleanup() {
  if [[ "$attached" == yes ]]; then
    hdiutil detach "$mount" >/dev/null || return
  fi
  rm -rf "$stage"
}
trap cleanup EXIT

image="$stage/image"
contents="$image/Sliploom.app/Contents"
mkdir -p "$(dirname "$package")" "$contents/MacOS" "$contents/Resources" "$mount"
install -m 755 "$binary" "$contents/MacOS/sliploom"
install -m 644 "$notice" "$contents/Resources/Roboto-OFL.txt"
install -m 644 packaging/Info.plist "$contents/Info.plist"
plutil -insert CFBundleShortVersionString -string "$version" "$contents/Info.plist"
plutil -insert CFBundleVersion -string "$version" "$contents/Info.plist"
ln -s /Applications "$image/Applications"

hdiutil create -volname Sliploom -srcfolder "$image" -fs HFS+ -format UDZO "$package"
test -s "$package"
hdiutil verify "$package"
hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$package"
attached=yes
contents="$mount/Sliploom.app/Contents"
test -x "$contents/MacOS/sliploom"
test "$(lipo -archs "$contents/MacOS/sliploom")" = arm64
cmp "$binary" "$contents/MacOS/sliploom"
cmp "$notice" "$contents/Resources/Roboto-OFL.txt"
plutil -lint "$contents/Info.plist"
test "$(plutil -extract CFBundleIdentifier raw -o - "$contents/Info.plist")" = org.projectseam.sliploom
test "$(plutil -extract CFBundleExecutable raw -o - "$contents/Info.plist")" = sliploom
test "$(plutil -extract CFBundleShortVersionString raw -o - "$contents/Info.plist")" = "$version"
test "$(plutil -extract CFBundleVersion raw -o - "$contents/Info.plist")" = "$version"
test "$(readlink "$mount/Applications")" = /Applications
hdiutil detach "$mount" >/dev/null
attached=no

cd "$(dirname "$package")"
filename=$(basename "$package")
shasum -a 256 -- "$filename" > "$filename.sha256"
