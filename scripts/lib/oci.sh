#!/usr/bin/env bash
# The blob store and image index both image builders write.

# Moves a file into the blobs by its digest and prints a descriptor for it.
oci_blob() {
  local out=$1 type=$2 file=$3 digest size
  digest=$(sha256sum "$file" | cut -d' ' -f1)
  size=$(stat -c %s "$file")
  mv "$file" "$out/blobs/sha256/$digest"
  jq -nc --arg type "$type" --arg digest "sha256:$digest" --argjson size "$size" \
    '{mediaType: $type, digest: $digest, size: $size}'
}

oci_index() {
  local out=$1 work=$2 tag=$3 image=$4 index
  shift 4
  printf '%s\n' "$@" | jq -sc '{
      schemaVersion: 2, mediaType: "application/vnd.oci.image.index.v1+json", manifests: .
    }' > "$work/index.json"
  index=$(oci_blob "$out" application/vnd.oci.image.index.v1+json "$work/index.json")
  jq -nc --argjson index "$index" --arg tag "$tag" --arg name "$image:$tag" '{
      schemaVersion: 2, mediaType: "application/vnd.oci.image.index.v1+json",
      manifests: [$index + {annotations: {
        "org.opencontainers.image.ref.name": $tag, "io.containerd.image.name": $name
      }}]
    }' > "$out/index.json"
  printf '%s\n' "$index"
}