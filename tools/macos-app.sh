#!/usr/bin/env bash
# Ehitab iview.app paki: target/release/iview.app (ikoon Apple'i ruudustikus).
#
#   tools/macos-app.sh                 # pakk target/release/iview.app
#   tools/macos-app.sh --install       # ja koopia /Applications kausta
#   IVIEW_UNIVERSAL=1 tools/macos-app.sh   # Apple Silicon + Intel ühes binaaris
#   IVIEW_SIGN="Apple Development: …" tools/macos-app.sh
#   IVIEW_BUNDLE_ID=ee.minu.iview tools/macos-app.sh
#
# Paki kaudu küsib macOS kaitstud kaustade (Töölaud, Dokumendid,
# Allalaadimised, kettad) luba iview enda nimel; terminalist käivitatud
# binaar saab ainult terminali loa. Pakk registreeritakse LaunchServicesis,
# nii et Finderi „Open With" pakub iview'd piltidele.
#
# Allkiri: IVIEW_SIGN, muidu võtmehoidjast esimene „Developer ID
# Application" (levitamiseks), siis „Apple Development" (oma masinas); kui
# kumbagi pole, ad-hoc („-"). Sertifikaadi nime skriptis pole — iga ehitaja
# saab oma allkirja, privaatvõti jääb tema masinasse. Ad-hoc allkiri muutub
# iga ehitusega ja macOS küsib kaustade luba siis uuesti.
set -euo pipefail

if [ "$(uname)" != "Darwin" ]; then
    echo "macos-app.sh runs on macOS only (it needs sips, iconutil and codesign)." >&2
    exit 1
fi

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

install=0
for arg in "$@"; do
    case "$arg" in
        --install) install=1 ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "Unknown option: $arg" >&2; exit 2 ;;
    esac
done

bundle_id="${IVIEW_BUNDLE_ID:-local.iview}"
# Esimene sobiv allkirjastamise identiteet võtmehoidjas (nimi jutumärkide vahel).
find_identity() {
    security find-identity -v -p codesigning 2>/dev/null \
        | sed -n "s/^ *[0-9]*) [0-9A-F]* \"\($1:.*\)\"\$/\1/p" | head -1
}
sign="${IVIEW_SIGN:-}"
if [ -z "$sign" ]; then
    sign="$(find_identity "Developer ID Application")"
fi
if [ -z "$sign" ]; then
    sign="$(find_identity "Apple Development")"
fi
sign="${sign:--}"

# Binaar: praeguse masina arhitektuur või universaalne (lipo).
if [ "${IVIEW_UNIVERSAL:-0}" = "1" ]; then
    rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null
    cargo build --release --locked --target aarch64-apple-darwin
    cargo build --release --locked --target x86_64-apple-darwin
    bin="target/release/iview-universal"
    mkdir -p target/release
    lipo -create -output "$bin" \
        target/aarch64-apple-darwin/release/iview \
        target/x86_64-apple-darwin/release/iview
else
    cargo build --release --locked
    bin="target/release/iview"
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
# Ehituse number: commit'ide arv; git-ita kuupäev ja kellaaeg.
build="$(git rev-list --count HEAD 2>/dev/null || date +%Y%m%d%H%M)"

app="target/release/iview.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/iview"
sed -e "s/@BUNDLE_ID@/$bundle_id/" -e "s/@VERSION@/$version/" -e "s/@BUILD@/$build/" \
    assets/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null
printf 'APPL????' > "$app/Contents/PkgInfo"

# Ikoon: programm joonistab ikoonikomplekti ise (sama joonistus mis aknal ja
# Dockis, Apple'i ruudustikus) → .icns Maci enda tööriistaga.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
"$bin" --export-iconset "$tmp/iview.iconset" >/dev/null
iconutil -c icns "$tmp/iview.iconset" -o "$app/Contents/Resources/iview.icns"

codesign --force --sign "$sign" --identifier "$bundle_id" "$app"
codesign --verify --strict "$app"

# LaunchServices: „Open With" ja ikoon kohe, mitte alles pärast Finderi
# taaskäivitust.
lsregister="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
[ -x "$lsregister" ] && "$lsregister" -f "$app" || true

if [ "$install" = "1" ]; then
    dest="/Applications/iview.app"
    rm -rf "$dest"
    ditto "$app" "$dest"
    [ -x "$lsregister" ] && "$lsregister" -f "$dest" || true
    app="$dest"
fi

echo "Done: $app ($version, build $build, $bundle_id, signature: $sign)"
