#!/usr/bin/env bash
# Adds the built example games to the distroless Node.js images.
set -euo pipefail
shopt -s inherit_errexit

if [[ $# != 5 ]]; then
  echo "usage: bash scripts/games-image.sh tag image revision out base" >&2
  exit 1
fi
tag=$1
image=$2
revision=$3
out=$4
base_image=$5
source "$(dirname -- "${BASH_SOURCE[0]}")/lib/oci.sh"

rm -rf "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
read_blob() { cat "$out/blobs/sha256/${1#sha256:}"; }
# The base, one manifest per architecture, in the layout this writes to.
for arch in amd64 arm64; do
  skopeo copy --quiet --override-os linux --override-arch "$arch" \
    "docker://$base_image" "oci:$out:$arch"
done
# One layer: /games, what serve.js serves and runs, without the tests or the Rust
# sources, owned by root and with fixed times, so the same games give the same digest.
mkdir -p "$work/root"
tar --create --directory=. --exclude=games/test --exclude='games/*/src' \
  --exclude='games/*/Cargo.*' games | tar --extract --directory="$work/root"
tar --create --format=posix --sort=name --owner=0 --group=0 --numeric-owner \
  --mode=a+rX,go-w --mtime=@0 --pax-option=delete=atime,delete=ctime \
  --file="$work/layer.tar" --directory="$work/root" games
diff_id=sha256:$(sha256sum "$work/layer.tar" | cut -d' ' -f1)
gzip -n -9 "$work/layer.tar"
layer=$(oci_blob "$out" application/vnd.oci.image.layer.v1.tar+gzip "$work/layer.tar.gz")
created=$(git log -1 --format=%cI 2>/dev/null || date -u +%FT%TZ)
manifests=()
for arch in amd64 arm64; do
  base=$(jq -c --arg arch "$arch" \
    '.manifests[] | select(.annotations["org.opencontainers.image.ref.name"] == $arch)' \
    "$out/index.json")
  manifest=$(read_blob "$(jq -r .digest <<< "$base")")
  read_blob "$(jq -r .config.digest <<< "$manifest")" | jq -c \
    --arg diff_id "$diff_id" --arg created "$created" --arg version "$tag" \
    --arg revision "$revision" '
      .created = $created
      | .rootfs.diff_ids += [$diff_id]
      | .history += [{created: $created, created_by: "just games-image"}]
      | .config.WorkingDir = "/games"
      | .config.Cmd = ["/games/serve.js"]
      | .config.Env = ((.config.Env // []) + ["PORT=8082"])
      | .config.ExposedPorts = {"8082/tcp": {}}
      | .config.Labels = ((.config.Labels // {}) + {
          "org.opencontainers.image.source": "https://github.com/vehagn/innby",
          "org.opencontainers.image.version": $version,
          "org.opencontainers.image.revision": $revision,
          "org.opencontainers.image.created": $created
        })' > "$work/config.json"
  config=$(oci_blob "$out" application/vnd.oci.image.config.v1+json "$work/config.json")
  jq -c --argjson config "$config" --argjson layer "$layer" \
    '.config = $config | .layers += [$layer]' <<< "$manifest" > "$work/manifest.json"
  manifests+=("$(oci_blob "$out" application/vnd.oci.image.manifest.v1+json "$work/manifest.json" \
    | jq -c --arg arch "$arch" '. + {platform: {architecture: $arch, os: "linux"}}')")
done
index=$(oci_index "$out" "$work" "$tag" "$image" "${manifests[@]}")
echo "$image:$tag $(jq -r .digest <<< "$index")"