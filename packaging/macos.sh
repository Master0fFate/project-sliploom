set -euo pipefail

mode=package
if [[ ${1:-} == --validate-mtp-runtime && $# == 2 ]]; then
  mode=validate-mtp
  version=$2
elif [[ $# == 2 && ${1:-} != --* ]]; then
  version=$1
  package=$2
else
  printf 'Usage: bash packaging/macos.sh VERSION OUTPUT.dmg\n       bash packaging/macos.sh --validate-mtp-runtime VERSION\n' >&2
  exit 2
fi

prefix=${SLIPLOOM_MTP_PREFIX:?set SLIPLOOM_MTP_PREFIX to the macos-build-mtp.sh build prefix}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { printf 'Expected a numeric VERSION\n' >&2; exit 2; }
bash packaging/macos-build-mtp.sh --verify "$prefix"
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
mkdir -p "$contents/MacOS" "$contents/Resources/MTP-Sources" "$mount"
install -m 755 "$binary" "$contents/MacOS/sliploom"
cmp "$binary" "$contents/MacOS/sliploom"
install -m 644 "$notice" "$contents/Resources/Roboto-OFL.txt"
install -m 644 "$prefix/sources/"* "$contents/Resources/MTP-Sources/"
install -m 644 packaging/Info.plist "$contents/Info.plist"
plutil -insert CFBundleShortVersionString -string "$version" "$contents/Info.plist"
plutil -insert CFBundleVersion -string "$version" "$contents/Info.plist"
ln -s /Applications "$image/Applications"

/usr/bin/python3 -I packaging/macos-runtime.py stage "$contents" "$prefix"
/usr/bin/python3 -I packaging/macos-runtime.py verify "$contents" "$version"

dmg="$stage/Sliploom.dmg"
hdiutil create -volname Sliploom -srcfolder "$image" -fs HFS+ -format UDZO "$dmg"
test -s "$dmg"
hdiutil verify "$dmg"
hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$dmg"
attached=yes
staged=$contents
contents="$mount/Sliploom.app/Contents"
test -x "$contents/MacOS/sliploom"
cmp "$staged/MacOS/sliploom" "$contents/MacOS/sliploom"
cmp "$staged/Info.plist" "$contents/Info.plist"
cmp "$notice" "$contents/Resources/Roboto-OFL.txt"
for dylib in "$staged/Frameworks/"*.dylib; do
  cmp "$dylib" "$contents/Frameworks/${dylib##*/}"
done
diff -qr "$staged/Resources/MTP-Sources" "$contents/Resources/MTP-Sources"
/usr/bin/python3 -I packaging/macos-runtime.py verify "$contents" "$version"
plutil -lint "$contents/Info.plist"
test "$(plutil -extract CFBundleIdentifier raw -o - "$contents/Info.plist")" = org.projectseam.sliploom
test "$(plutil -extract CFBundleExecutable raw -o - "$contents/Info.plist")" = sliploom
test "$(plutil -extract CFBundleShortVersionString raw -o - "$contents/Info.plist")" = "$version"
test "$(plutil -extract CFBundleVersion raw -o - "$contents/Info.plist")" = "$version"
test "$(readlink "$mount/Applications")" = /Applications
hdiutil detach "$mount" >/dev/null
attached=no

if [[ "$mode" == validate-mtp ]]; then
  printf '%s\n' 'Mounted MTP runtime verified; temporary DMG will be removed.'
  exit 0
fi

mkdir -p "$(dirname "$package")"
cd "$(dirname "$package")"
filename=$(basename "$package")
test ! -e "$filename"
test ! -L "$filename"
test ! -e "$filename.sha256"
test ! -L "$filename.sha256"
mv -- "$dmg" "$filename"
shasum -a 256 -- "$filename" > "$filename.sha256"
