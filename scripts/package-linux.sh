#!/usr/bin/env bash
set -euo pipefail
arch=$(uname -m)
case "$arch" in x86_64|aarch64) ;; *) echo "Unsupported architecture: $arch" >&2; exit 1 ;; esac
root=$(pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/RustTalk.AppDir"
mkdir -p "$appdir/usr/bin" "$appdir/usr/lib/gstreamer-1.0" "$appdir/usr/libexec/gstreamer-1.0" dist
python3 scripts/fetch-release-tool.py linuxdeploy/linuxdeploy 1-alpha-20251107-1 "linuxdeploy-$arch.AppImage" "$work/linuxdeploy"
python3 scripts/fetch-release-tool.py AppImage/appimagetool 1.9.1 "appimagetool-$arch.AppImage" "$work/appimagetool"
python3 scripts/fetch-release-tool.py AppImage/type2-runtime 20251108 "runtime-$arch" "$work/runtime"
cp target/release/rusttalk "$appdir/usr/bin/"
plugins=$(pkg-config --variable=pluginsdir gstreamer-1.0)
# Include dynamically loaded capture plugins and their transitive dependencies.
for plugin in app coreelements playback typefindfunctions videoconvertscale video4linux2 jpeg pipewire; do
    cp "$plugins/libgst$plugin.so" "$appdir/usr/lib/gstreamer-1.0/"
done
scanner=$(pkg-config --variable=libexecdir gstreamer-1.0)/gstreamer-1.0/gst-plugin-scanner
cp "$scanner" "$appdir/usr/libexec/gstreamer-1.0/"
libdir=$(pkg-config --variable=libdir libpipewire-0.3)
cp -r "$libdir/spa-0.2" "$appdir/usr/lib/"
alsa=$(pkg-config --variable=libdir alsa)/alsa-lib
mkdir -p "$appdir/usr/lib/alsa-lib"
# Only the PulseAudio bridge is needed; optional host plugins can have missing dependencies.
cp "$alsa/libasound_module_pcm_pulse.so" "$alsa/libasound_module_ctl_pulse.so" "$alsa/libasound_module_conf_pulse.so" "$appdir/usr/lib/alsa-lib/"
for plugin in pcm_pipewire ctl_pipewire; do
    if test -f "$alsa/libasound_module_$plugin.so"; then
        cp "$alsa/libasound_module_$plugin.so" "$appdir/usr/lib/alsa-lib/"
    fi
done
args=()
while IFS= read -r -d '' library; do args+=(--library "$library"); done < <(find "$appdir/usr/lib" -type f -name '*.so*' -print0)
# linuxdeploy resolves ELF dependencies; host GPU drivers and glibc remain host-owned.
NO_STRIP=1 APPIMAGE_EXTRACT_AND_RUN=1 "$work/linuxdeploy" --appdir "$appdir" \
    --executable "$appdir/usr/bin/rusttalk" --executable "$appdir/usr/libexec/gstreamer-1.0/gst-plugin-scanner" \
    --desktop-file packaging/linux/org.rusttalk.RustTalk.desktop --icon-file packaging/linux/org.rusttalk.RustTalk.svg "${args[@]}"
rm -f "$appdir/AppRun" # linuxdeploy creates a symlink to the executable
cp packaging/linux/AppRun "$appdir/AppRun"
chmod +x "$appdir/AppRun"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$arch" "$work/appimagetool" --runtime-file "$work/runtime" "$appdir" "$work/rusttalk-linux-$arch.AppImage"
# Replace the artifact without truncating an image that may still be mounted.
cp "$work/rusttalk-linux-$arch.AppImage" "$root/dist/.rusttalk-linux-$arch.AppImage.new"
mv -f "$root/dist/.rusttalk-linux-$arch.AppImage.new" "$root/dist/rusttalk-linux-$arch.AppImage"
APPIMAGE_EXTRACT_AND_RUN=1 "dist/rusttalk-linux-$arch.AppImage" --check-runtime
python3 scripts/test-appimage-update.py "dist/rusttalk-linux-$arch.AppImage"
cp target/release/rusttalk-server "dist/rusttalk-server-linux-$arch"
mkdir -p "$work/server"
cp target/release/rusttalk-server Server/server.toml "$work/server/"
cp -r Server/deploy "$work/server/"
tar -C "$work/server" -czf "dist/rusttalk-server-linux-$arch.tar.gz" .
(cd dist; sha256sum "rusttalk-linux-$arch.AppImage" > "rusttalk-linux-$arch.AppImage.sha256"; sha256sum "rusttalk-server-linux-$arch" > "rusttalk-server-linux-$arch.sha256")
