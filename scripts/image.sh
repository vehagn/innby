#!/usr/bin/env bash
# Packages the static binaries as an OCI image and a Docker-loadable archive.
set -euo pipefail
shopt -s inherit_errexit

if [[ $# != 5 ]]; then
  echo "usage: bash scripts/image.sh tag image revision out target_dir" >&2
  exit 1
fi
tag=$1
image=$2
revision=$3
out=$4
target_dir=$5
source "$(dirname -- "${BASH_SOURCE[0]}")/lib/oci.sh"

rm -rf "$out"
mkdir -p "$out/blobs/sha256"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
created=$(git log -1 --format=%cI 2>/dev/null || date -u +%FT%TZ)
case "$(uname -m)" in x86_64) native=amd64 ;; *) native=arm64 ;; esac
manifests=()
for arch in amd64 arm64; do
  case "$arch" in
    amd64) target=x86_64-unknown-linux-musl ;;
    arm64) target=aarch64-unknown-linux-musl ;;
  esac
  # One layer: /innby, owned by root, with fixed times so the same binary gives the same digest.
  mkdir -p "$work/$arch"
  install -m 0555 "$target_dir/$target/release/innby" "$work/$arch/innby"
  tar --create --format=posix --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime=@0 --pax-option=delete=atime,delete=ctime \
    --file="$work/layer.tar" --directory="$work/$arch" innby
  diff_id=sha256:$(sha256sum "$work/layer.tar" | cut -d' ' -f1)
  gzip -n -9 "$work/layer.tar"
  layer=$(oci_blob "$out" application/vnd.oci.image.layer.v1.tar+gzip "$work/layer.tar.gz")
  jq -nc --arg arch "$arch" --arg diff_id "$diff_id" --arg created "$created" \
    --arg version "$tag" --arg revision "$revision" '{
      created: $created, architecture: $arch, os: "linux",
      config: {
        Entrypoint: ["/innby"], Cmd: ["serve"], User: "65532:65532",
        ExposedPorts: {"8080/tcp": {}},
        Labels: {
          "org.opencontainers.image.source": "https://github.com/vehagn/innby",
          "org.opencontainers.image.version": $version,
          "org.opencontainers.image.revision": $revision,
          "org.opencontainers.image.created": $created
        }
      },
      rootfs: {type: "layers", diff_ids: [$diff_id]}
    }' > "$work/config.json"
  config=$(oci_blob "$out" application/vnd.oci.image.config.v1+json "$work/config.json")
  # Docker before 25 loads only `docker save`'s manifest.json, so it gets this machine's image.
  if [[ "$arch" == "$native" ]]; then
    jq -nc --argjson config "$config" --argjson layer "$layer" --arg name "$image:$tag" \
      '[{Config: ("blobs/" + ($config.digest | sub(":"; "/"))), RepoTags: [$name],
         Layers: ["blobs/" + ($layer.digest | sub(":"; "/"))]}]' > "$out/manifest.json"
  fi
  jq -nc --argjson config "$config" --argjson layer "$layer" '{
      schemaVersion: 2, mediaType: "application/vnd.oci.image.manifest.v1+json",
      config: $config, layers: [$layer]
    }' > "$work/manifest.json"
  manifests+=("$(oci_blob "$out" application/vnd.oci.image.manifest.v1+json "$work/manifest.json" \
    | jq -c --arg arch "$arch" '. + {platform: {architecture: $arch, os: "linux"}}')")
done
index=$(oci_index "$out" "$work" "$tag" "$image" "${manifests[@]}")
echo '{"imageLayoutVersion":"1.0.0"}' > "$out/oci-layout"
tar --create --file="$out.tar" --directory="$out" .
echo "$image:$tag $(jq -r .digest <<< "$index")"
echo "$out.tar: \`just up $tag\` on the host loads and runs it"