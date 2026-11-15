#!/usr/bin/env bash
# Keep downloadable program versions aligned with the release tag.
set -euo pipefail

tag="${1:-}"
metadata="$(cargo metadata --locked --no-deps --format-version 1)"
version="$(jq -r '.packages[] | select(.name == "foilfern-app") | .version' <<< "$metadata")"
for package in foilfern-cli foilfern-mcp; do
  actual="$(jq -r --arg package "$package" '.packages[] | select(.name == $package) | .version' <<< "$metadata")"
  if [[ "$actual" != "$version" ]]; then
    echo "$package version $actual differs from foilfern-app $version" >&2
    exit 1
  fi
done
if [[ -n "$tag" && "$tag" != "v$version" ]]; then
  echo "Release tag $tag does not match binary version v$version" >&2
  exit 1
fi
printf '%s\n' "$version"
