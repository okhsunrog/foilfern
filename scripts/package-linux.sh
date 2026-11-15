#!/usr/bin/env bash
# Package the three release programs and a GUI AppImage on Ubuntu x86_64.
# Build first; all staging and diagnostic files stay in the caller's directories.
set -euo pipefail

work="${1:?usage: package-linux.sh WORK_DIRECTORY OUTPUT_DIRECTORY}"
output="${2:?usage: package-linux.sh WORK_DIRECTORY OUTPUT_DIRECTORY}"
mkdir -p "$work" "$output"
work="$(realpath "$work")"
output="$(realpath "$output")"
mkdir -p "$work/tmp"
export TMPDIR="$work/tmp"
version="$(bash scripts/check-release-version.sh)"
target="$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.target_directory')"
name="foilfern-$version-linux-x86_64"
bundle="$work/$name"
appdir="$work/AppDir"
mkdir -p "$bundle/bin" "$appdir/usr/bin" "$appdir/usr/share/doc/foilfern"

for binary in foilfern foilfern-cli foilfern-mcp; do
  install -m 755 "$target/release/$binary" "$bundle/bin/$binary"
  strip --strip-unneeded "$bundle/bin/$binary"
  install -m 755 "$bundle/bin/$binary" "$appdir/usr/bin/$binary"
done
install -m 644 LICENSE.txt "$bundle/LICENSE.txt"
install -m 644 docs/releases.md "$bundle/README.md"
printf 'version=%s\ncommit=%s\n' "$version" "$(git rev-parse HEAD)" > "$bundle/BUILD.txt"
# Include the licenses accompanying the embedded fonts and icon sets.
mkdir -p "$bundle/licenses"
install -m 644 crates/foilfern-app/ui/resources/share/librepcb/fonts/LICENSE "$bundle/licenses/noto-fonts.txt"
install -m 644 crates/foilfern-app/ui/resources/libs/bootstrap-icons/LICENSE "$bundle/licenses/bootstrap-icons.txt"
install -m 644 crates/foilfern-app/ui/resources/libs/font-awesome/LICENSE.txt "$bundle/licenses/font-awesome.txt"
cp -r "$bundle/licenses" "$bundle/LICENSE.txt" "$bundle/BUILD.txt" "$appdir/usr/share/doc/foilfern/"
install -m 644 docs/releases.md "$appdir/usr/share/doc/foilfern/README.md"
tar -czf "$output/$name.tar.gz" -C "$work" "$name"

deploy="$work/linuxdeploy-x86_64.AppImage"
curl --fail --location --retry 3 --output "$deploy" \
  https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-x86_64.AppImage
echo "c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d  $deploy" | sha256sum --check
chmod +x "$deploy"

cat > "$work/foilfern.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Foilfern
Comment=Schematic and PCB editor
Exec=foilfern %F
Icon=foilfern
Categories=Development;Electronics;
Terminal=false
DESKTOP
cat > "$work/AppRun" <<'APPRUN'
#!/usr/bin/env bash
set -euo pipefail
APPDIR="${APPDIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)}"
export LD_LIBRARY_PATH="$APPDIR/usr/lib:$APPDIR/usr/lib64${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
binary=foilfern
case "${1:-}" in
  --run-cli) binary=foilfern-cli; shift ;;
  --run-mcp) binary=foilfern-mcp; shift ;;
esac
exec "$APPDIR/usr/bin/$binary" "$@"
APPRUN
chmod +x "$work/AppRun"

# Winit loads these libraries at runtime; they are not all visible to ldd.
# The graphics driver and Vulkan/OpenGL loader remain host responsibilities.
libraries=()
for library in libfontconfig.so.1 libfreetype.so.6 libxkbcommon.so.0 \
  libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1 \
  libX11.so.6 libXcursor.so.1 libXi.so.6 libXrandr.so.2 libxcb.so.1 \
  libXext.so.6 libXfixes.so.3; do
  path="$(ldconfig -p | awk -v name="$library" '$1 == name && /x86-64/ && !found { print $NF; found=1 } END { if (!found) exit 1 }')"
  libraries+=(--library "$path")
done
export APPIMAGE_EXTRACT_AND_RUN=1
export ARCH=x86_64
export LDAI_OUTPUT="$output/Foilfern-$version-x86_64.AppImage"
export LDAI_VERSION="$version"
export LDAI_NO_APPSTREAM=1
"$deploy" --appdir "$appdir" \
  --executable "$appdir/usr/bin/foilfern" \
  --executable "$appdir/usr/bin/foilfern-cli" \
  --executable "$appdir/usr/bin/foilfern-mcp" \
  --desktop-file "$work/foilfern.desktop" \
  --icon-file docs/assets/foilfern.svg \
  --custom-apprun "$work/AppRun" "${libraries[@]}" --output appimage

# Test the downloaded forms, including a real headless UI frame, not just --help.
mkdir -p "$work/unpacked"
tar -xzf "$output/$name.tar.gz" -C "$work/unpacked"
for binary in foilfern foilfern-cli foilfern-mcp; do
  "$work/unpacked/$name/bin/$binary" --version
done
"$LDAI_OUTPUT" --version
"$LDAI_OUTPUT" --run-cli --version
"$LDAI_OUTPUT" --run-mcp call server_info '{}' > "$work/server-info.jsonl"
jq -e '.response.structuredContent.result.build | .cargo_profile == "release" and .tracked_source_state == "clean"' "$work/server-info.jsonl"
jq -n --arg directory "$work/smoke-project" '[
  {tool:"project_create",arguments:{name:"Smoke",directory:$directory}},
  {tool:"project_save"}
]' > "$work/smoke-calls.json"
"$LDAI_OUTPUT" --run-mcp batch "$work/smoke-calls.json" > "$work/smoke-results.jsonl"
test -s "$work/smoke-project/resources/fontobene/newstroke.bene"
project="$(jq -r 'select(.tool == "project_create") | .response.structuredContent.result.path' "$work/smoke-results.jsonl")"
"$LDAI_OUTPUT" --workspace "$work/smoke-workspace" --project "$project" --tab board \
  --screenshot "$work/smoke.png" --size 640x480
test -s "$work/smoke.png"
timeout --kill-after=5s 30s xvfb-run -a bash scripts/smoke-linux-desktop.sh "$LDAI_OUTPUT" "$work" "$project"
(cd "$output" && sha256sum "$name.tar.gz" "Foilfern-$version-x86_64.AppImage" > SHA256SUMS)
