#!/usr/bin/env bash
# Installs Ferrix from this unpacked release for the current user, so desktops list it with
# its icon and offer it for PDFs:
#   <prefix>/lib/ferrix/                    ferrix-app, ferrix, libpdfium.so, licenses
#   <prefix>/bin/ferrix-app, ferrix         links to the above
#   <prefix>/share/applications/            io.github.sbrosgol.ferrix.desktop
#   <prefix>/share/icons/hicolor/scalable/  io.github.sbrosgol.ferrix.svg
#
# Usage: ./install.sh [--uninstall] [prefix]    (prefix defaults to ~/.local)
set -euo pipefail

id=io.github.sbrosgol.ferrix
uninstall=false
if [ "${1:-}" = --uninstall ]; then
  uninstall=true
  shift
fi
prefix=${1:-$HOME/.local}
here=$(cd "$(dirname "$0")" && pwd)

lib=$prefix/lib/ferrix
desktop=$prefix/share/applications/$id.desktop
icon=$prefix/share/icons/hicolor/scalable/apps/$id.svg

# Let the desktop notice the change now rather than at next login. Both tools are optional.
refresh() {
  command -v update-desktop-database >/dev/null &&
    update-desktop-database -q "$prefix/share/applications" || true
  command -v gtk-update-icon-cache >/dev/null &&
    gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
}

if $uninstall; then
  rm -rf "$lib"
  rm -f "$prefix/bin/ferrix-app" "$prefix/bin/ferrix" "$desktop" "$icon"
  refresh
  echo "Removed Ferrix from $prefix"
  exit 0
fi

# The desktop entry runs the app by its full path: ~/.local/bin is often not on the PATH
# that desktop sessions start programs with. Exec quoting can't carry these characters.
case $lib in
  *[\"\`\$\\]*) echo "install.sh: the prefix can't contain \", \`, \$ or \\" >&2; exit 1 ;;
esac

rm -rf "$lib"
mkdir -p "$lib" "$prefix/bin" "$(dirname "$desktop")" "$(dirname "$icon")"
cp -R "$here"/. "$lib/"
rm -f "$lib/install.sh" "$lib/$id.desktop" "$lib/$id.svg"
# The app finds libpdfium.so beside its real path, so links into bin work.
ln -sf "$lib/ferrix-app" "$prefix/bin/ferrix-app"
ln -sf "$lib/ferrix" "$prefix/bin/ferrix"

while IFS= read -r line; do
  case $line in
    Exec=*) printf 'Exec="%s/ferrix-app" %%f\n' "$lib" ;;
    *) printf '%s\n' "$line" ;;
  esac
done <"$here/$id.desktop" >"$desktop"
cp "$here/$id.svg" "$icon"

refresh
echo "Installed Ferrix in $prefix"
