#!/usr/bin/env bash
# Builds release binaries for the host OS and packages them with PDFium into out/:
#   macOS    Ferrix-<version>-macos-universal.dmg  (Ferrix.app for Apple silicon and Intel)
#   Windows  Ferrix-<version>-windows-<arch>.zip
#   Linux    Ferrix-<version>-linux-<arch>.tar.gz
#
# Usage: dist/package.sh <version>
# Used by .github/workflows/release.yml; runs locally too (on Windows, from Git Bash).
set -euo pipefail

version=${1:?usage: dist/package.sh <version>}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
out=$root/out
work=$root/target/package
rm -rf "$work"
mkdir -p "$out" "$work"

# Ask rustc, not uname: Git Bash on Windows on ARM is an x64 program.
host=$(rustc -vV | sed -n 's/^host: //p')
case $host in
  aarch64-*) arch=arm64 ;;
  x86_64-*) arch=x64 ;;
  *) echo "unsupported host $host" >&2; exit 1 ;;
esac

# Licenses for Ferrix and for everything shipped inside it.
legal() {
  local dest=$1
  mkdir -p "$dest/third-party/pdfium" "$dest/third-party/lucide"
  cp LICENSE-MIT LICENSE-APACHE README.md "$dest/"
  cp -R vendor/pdfium/LICENSE vendor/pdfium/licenses "$dest/third-party/pdfium/"
  cp crates/ferrix-app/assets/icons/LICENSE "$dest/third-party/lucide/"
}

package_macos() {
  export MACOSX_DEPLOYMENT_TARGET=11.0
  local triples=(aarch64-apple-darwin x86_64-apple-darwin) pdfium=(mac-arm64 mac-x64)
  for i in 0 1; do
    rustup target add "${triples[$i]}"
    cargo xtask fetch-pdfium --platform "${pdfium[$i]}"
    cp vendor/pdfium/lib/libpdfium.dylib "$work/libpdfium-${pdfium[$i]}.dylib"
    cargo build --release --locked --target "${triples[$i]}" -p ferrix-app -p ferrix-cli
  done

  local app=$work/dmg/Ferrix.app
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources"
  for bin in ferrix-app ferrix; do
    lipo -create -output "$app/Contents/MacOS/$bin" \
      "target/aarch64-apple-darwin/release/$bin" "target/x86_64-apple-darwin/release/$bin"
  done
  lipo -create -output "$app/Contents/Frameworks/libpdfium.dylib" "$work"/libpdfium-mac-*.dylib
  sed "s/@VERSION@/$version/g" dist/macos/Info.plist >"$app/Contents/Info.plist"
  cargo xtask icns "$app/Contents/Resources/Ferrix.icns"
  legal "$app/Contents/Resources"

  # Ad-hoc signature: Apple silicon refuses to run unsigned code. Not a Developer ID
  # signature, so Gatekeeper still asks users to confirm the first launch.
  codesign --force --sign - "$app/Contents/Frameworks/libpdfium.dylib"
  codesign --force --sign - "$app/Contents/MacOS/ferrix"
  codesign --force --sign - "$app"
  codesign --verify --strict "$app"

  ln -s /Applications "$work/dmg/Applications"
  local dmg=$out/Ferrix-$version-macos-universal.dmg
  rm -f "$dmg"
  # hdiutil fails intermittently on CI runners ("Resource busy"); retry.
  for attempt in 1 2 3; do
    hdiutil create -volname "Ferrix $version" -srcfolder "$work/dmg" -format UDZO "$dmg" && break
    [ "$attempt" = 3 ] && exit 1
    sleep 5
  done
  echo "$dmg"
}

package_windows() {
  cargo xtask fetch-pdfium
  cargo build --release --locked -p ferrix-app -p ferrix-cli
  local name=Ferrix-$version-windows-$arch
  local dest=$work/$name
  mkdir -p "$dest"
  cp target/release/ferrix-app.exe target/release/ferrix.exe vendor/pdfium/bin/pdfium.dll "$dest/"
  legal "$dest"
  rm -f "$out/$name.zip"
  (cd "$work" && 7z a -tzip -bso0 "$out/$name.zip" "$name")
  echo "$out/$name.zip"
}

package_linux() {
  cargo xtask fetch-pdfium
  cargo build --release --locked -p ferrix-app -p ferrix-cli
  local name=Ferrix-$version-linux-$arch
  local dest=$work/$name
  mkdir -p "$dest"
  cp target/release/ferrix-app target/release/ferrix vendor/pdfium/lib/libpdfium.so "$dest/"
  # The desktop entry and icon, and the script that installs them with the app.
  cp dist/linux/install.sh dist/linux/io.github.sbrosgol.ferrix.desktop "$dest/"
  cp crates/ferrix-app/assets/app-icon.svg "$dest/io.github.sbrosgol.ferrix.svg"
  legal "$dest"
  tar -C "$work" -czf "$out/$name.tar.gz" "$name"
  echo "$out/$name.tar.gz"
}

case $(uname -s) in
  Darwin) package_macos ;;
  Linux) package_linux ;;
  MINGW* | MSYS* | CYGWIN*) package_windows ;;
  *) echo "unsupported OS $(uname -s)" >&2; exit 1 ;;
esac
