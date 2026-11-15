#!/usr/bin/env bash
# Run inside Xvfb with an outer timeout. Exercise the real Winit window backend.
set -euo pipefail
image="${1:?AppImage path}"
work="${2:?work directory}"
project="${3:?project path}"
mkdir -p "$work/config" "$work/data" "$work/cache" "$work/runtime"
chmod 700 "$work/runtime"
export XDG_CONFIG_HOME="$work/config"
export XDG_DATA_HOME="$work/data"
export XDG_CACHE_HOME="$work/cache"
export XDG_RUNTIME_DIR="$work/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY WAYLAND_SOCKET DBUS_SESSION_BUS_ADDRESS AT_SPI_BUS_ADDRESS
export SLINT_BACKEND=winit-software
"$image" --workspace "$work/desktop-workspace" --project "$project" --tab board > "$work/desktop.log" 2>&1 &
child=$!
trap 'kill "$child" 2>/dev/null || true; wait "$child" 2>/dev/null || true' EXIT
for ((attempt = 0; attempt < 100; attempt++)); do
  window="$(xdotool search --onlyvisible --name 'Foilfern|LibrePCB' 2>/dev/null | head -n 1 || true)"
  if [[ -n "$window" ]]; then
    xdotool windowfocus --sync "$window"
    xdotool key --clearmodifiers ctrl+q
    wait "$child"
    trap - EXIT
    exit 0
  fi
  if ! kill -0 "$child" 2>/dev/null; then
    cat "$work/desktop.log" >&2
    exit 1
  fi
  sleep 0.1
done
cat "$work/desktop.log" >&2
echo 'No visible application window appeared' >&2
exit 1
