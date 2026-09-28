#!/usr/bin/env bash
set -euo pipefail

asset_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
icon_tmp="$(mktemp -d)"
trap 'rm -rf "$icon_tmp"' EXIT
iconset="$icon_tmp/tusklet.iconset"
mkdir "$iconset"

for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$asset_dir/tusklet.png" \
    --out "$iconset/icon_${size}x${size}.png" >/dev/null
  retina_size=$((size * 2))
  sips -z "$retina_size" "$retina_size" "$asset_dir/tusklet.png" \
    --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done

iconutil --convert icns "$iconset" --output "$asset_dir/tusklet.icns"
