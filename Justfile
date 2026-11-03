set dotenv-load

target_dir := env("CARGO_TARGET_DIR", justfile_directory() / "target")
image := env("INNBY_IMAGE", "ghcr.io/vehagn/innby")
revision := `git rev-parse HEAD 2>/dev/null || echo unknown`
# Where `just image` writes the OCI image layout; inside the workspace, so the host sees it.
oci_dir := justfile_directory() / "target/oci"
# The container engine for `just load` and `just up`, e.g. ENGINE=podman. Any Compose
# implementation works for `just up`, e.g. COMPOSE=docker-compose.
engine := env("ENGINE", "docker")
compose := env("COMPOSE", engine + " compose")
dev_compose := compose + " --file .devcontainer/compose.yaml --file .devcontainer/compose.ports.yaml"

# List available recipes
default:
    @just --list

# Run the public site (:8080), the admin UI (:8081) and the mailer (reads .env, see .env.example)
run: migrate
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --quiet
    trap 'kill 0' EXIT
    cargo run --quiet -- admin &
    cargo run --quiet -- mailer &
    cargo run --quiet -- serve

# Create an invitation and print the guest's link
invite name max_party_size="1": migrate
    cargo run --quiet -- invite "{{ name }}" {{ max_party_size }}

# Apply the retention rules once, as the hourly CronJob does
janitor: migrate
    cargo run --quiet -- janitor

# Hash an admin password for INNBY_ADMIN_PASSWORD_HASH (prompts without echo)
hash-password:
    #!/usr/bin/env bash
    set -euo pipefail
    read -rsp "Password: " password
    echo >&2
    printf %s "$password" | cargo run --quiet -- hash-password

# Run all tests; database tests get a throwaway database each
test: migrate
    cargo test --locked

# Apply the migrations to $DATABASE_URL (the query macros check against it at compile time)
migrate:
    sqlx migrate run

# Regenerate the offline query data in .sqlx/ (commit it)
prepare: migrate
    cargo sqlx prepare -- --all-targets

# Check formatting, lints and dependencies
lint:
    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo sqlx prepare --check -- --all-targets
    cargo deny check

# Format the code
fmt:
    cargo fmt

# Build static musl release binaries for amd64 and arm64 (offline, from .sqlx/)
build:
    SQLX_OFFLINE=true cargo zigbuild --locked --release \
      --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl

# Write the amd64 and arm64 images as one OCI image index to target/oci (plain files, no build tool)
image tag="dev": build
    #!/usr/bin/env bash
    set -euo pipefail
    out="{{ oci_dir }}"
    rm -rf "$out" && mkdir -p "$out/blobs/sha256"
    work=$(mktemp -d) && trap 'rm -rf "$work"' EXIT
    # Moves a file into the blobs by its digest and prints a descriptor for it.
    blob() {
      local digest size
      digest=$(sha256sum "$2" | cut -d' ' -f1)
      size=$(stat -c %s "$2")
      mv "$2" "$out/blobs/sha256/$digest"
      jq -nc --arg type "$1" --arg digest "sha256:$digest" --argjson size "$size" \
        '{mediaType: $type, digest: $digest, size: $size}'
    }
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
      install -m 0555 "{{ target_dir }}/$target/release/innby" "$work/$arch/innby"
      tar --create --format=posix --sort=name --owner=0 --group=0 --numeric-owner \
        --mtime=@0 --pax-option=delete=atime,delete=ctime \
        --file="$work/layer.tar" --directory="$work/$arch" innby
      diff_id=sha256:$(sha256sum "$work/layer.tar" | cut -d' ' -f1)
      gzip -n -9 "$work/layer.tar"
      layer=$(blob application/vnd.oci.image.layer.v1.tar+gzip "$work/layer.tar.gz")
      jq -nc --arg arch "$arch" --arg diff_id "$diff_id" --arg created "$created" \
        --arg version "{{ tag }}" --arg revision "{{ revision }}" '{
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
      config=$(blob application/vnd.oci.image.config.v1+json "$work/config.json")
      # Docker before 25 loads only `docker save`'s manifest.json, so it gets this machine's image.
      if [[ "$arch" == "$native" ]]; then
        jq -nc --argjson config "$config" --argjson layer "$layer" --arg name "{{ image }}:{{ tag }}" \
          '[{Config: ("blobs/" + ($config.digest | sub(":"; "/"))), RepoTags: [$name],
             Layers: ["blobs/" + ($layer.digest | sub(":"; "/"))]}]' > "$out/manifest.json"
      fi
      jq -nc --argjson config "$config" --argjson layer "$layer" '{
          schemaVersion: 2, mediaType: "application/vnd.oci.image.manifest.v1+json",
          config: $config, layers: [$layer]
        }' > "$work/manifest.json"
      manifests+=("$(blob application/vnd.oci.image.manifest.v1+json "$work/manifest.json" \
        | jq -c --arg arch "$arch" '. + {platform: {architecture: $arch, os: "linux"}}')")
    done
    printf '%s\n' "${manifests[@]}" | jq -sc '{
        schemaVersion: 2, mediaType: "application/vnd.oci.image.index.v1+json", manifests: .
      }' > "$work/index.json"
    index=$(blob application/vnd.oci.image.index.v1+json "$work/index.json")
    jq -nc --argjson index "$index" --arg tag "{{ tag }}" --arg name "{{ image }}:{{ tag }}" '{
        schemaVersion: 2, mediaType: "application/vnd.oci.image.index.v1+json",
        manifests: [$index + {annotations: {
          "org.opencontainers.image.ref.name": $tag, "io.containerd.image.name": $name
        }}]
      }' > "$out/index.json"
    echo '{"imageLayoutVersion":"1.0.0"}' > "$out/oci-layout"
    tar --create --file="{{ oci_dir }}.tar" --directory="$out" .
    echo "{{ image }}:{{ tag }} $(jq -r .digest <<< "$index")"
    echo "{{ oci_dir }}.tar: \`just load\` on the host puts it in Docker"

# Push the image index from `just image` under each tag given (`skopeo login ghcr.io` first)
push tag="dev" *also:
    #!/usr/bin/env bash
    set -euo pipefail
    for tag in {{ tag }} {{ also }}; do
      skopeo copy --all --preserve-digests "oci:{{ oci_dir }}:{{ tag }}" "docker://{{ image }}:$tag"
    done

# Load the image from `just image` into Docker or Podman, for `just up <tag>` (run on the host)
load:
    {{ engine }} load --input "{{ oci_dir }}.tar"

# Release a version: bump Cargo.toml if needed, tag v<version> (signed), push to build the image
release version:
    #!/usr/bin/env bash
    set -euo pipefail
    version="{{ trim_start_match(version, "v") }}"
    tag="v$version"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
      || { echo "not a version: $version (want e.g. 0.2.0)" >&2; exit 1; }
    [[ "$(git branch --show-current)" == main ]] || { echo "check out main first" >&2; exit 1; }
    [[ -z "$(git status --porcelain)" ]] || { echo "commit or stash your changes first" >&2; exit 1; }
    git fetch --quiet --tags origin main
    git merge-base --is-ancestor origin/main HEAD || { echo "main is behind origin/main" >&2; exit 1; }
    ! git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null || { echo "$tag already exists" >&2; exit 1; }
    current=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n1)
    if [[ "$current" != "$version" ]]; then
      perl -pi -e 's/^version = "[^"]*"/version = "'"$version"'"/ if /^version = / && !$done++' Cargo.toml
      perl -0pi -e 's/(name = "innby"\nversion = )"[^"]*"/$1"'"$version"'"/' Cargo.lock
      git commit --quiet --message "Release $tag" -- Cargo.toml Cargo.lock
      echo "Bumped $current → $version"
    fi
    git tag --sign --message "Innby $version" "$tag"
    git --no-pager log --oneline --decorate -1
    read -rp "Push main and $tag to origin? [y/N] " answer
    if [[ "$answer" == [yY] ]]; then
      git push --atomic origin main "$tag"
    else
      echo "Not pushed. Later: git push --atomic origin main $tag"
    fi

# Start the dev container with Postgres, Mailpit and your Vim settings, and open a shell in it; `just up` in there serves this checkout on localhost
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    test -f .env || cp .env.example .env
    # Your Vim and Neovim settings, read-only, but only those you have: Docker would
    # create the missing ones on the host as empty folders.
    mounts=()
    for config in .vimrc .vim .config/nvim; do
      if [[ -e "$HOME/$config" ]]; then mounts+=("\"$HOME/$config:/home/dev/$config:ro\""); fi
    done
    mkdir -p target
    editor=target/compose.editor.yaml
    printf 'services:\n  dev:\n    volumes: [%s]\n' "$(IFS=,; echo "${mounts[*]-}")" > "$editor"
    {{ dev_compose }} --file "$editor" up --detach --build --wait
    {{ dev_compose }} exec dev zsh

# Stop the dev container, keeping its database and build cache
dev-down:
    {{ dev_compose }} down

# Run a local copy of the published image (see compose.yaml), or of this checkout in the dev container: site :8080, admin :8081, mail :8025
up tag="latest":
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ -n "${INNBY_DEVCONTAINER:-}" ]]; then
      # In the dev container, which has no container engine: serve this checkout instead,
      # on the ports `just dev` publishes. Ctrl-C stops it.
      echo "http://localhost:8080  http://localhost:8081  http://localhost:8025"
      exec just run
    fi
    INNBY_TAG={{ tag }} {{ compose }} up --detach --pull missing
    echo "http://localhost:8080  http://localhost:8081  http://localhost:8025"

# Run the local copy's retention rules once
up-janitor:
    {{ compose }} run --rm janitor

# Stop the local copy, keeping its database (`down --volumes` deletes it)
down:
    {{ compose }} down

# Seal deploy/secrets.env with the cluster's key into deploy/sealed-secrets.yaml
seal:
    kubectl create secret generic innby --namespace innby \
      --from-env-file=deploy/secrets.env --dry-run=client --output=yaml \
      | kubeseal --controller-namespace sealed-secrets \
          --controller-name sealed-secrets-controller --format yaml \
      > deploy/sealed-secrets.yaml
    @echo "deploy/sealed-secrets.yaml: add it to deploy/kustomization.yaml"

# Render the Kubernetes manifests
manifests:
    kubectl kustomize deploy

# Open psql against the dev database
psql:
    psql "$DATABASE_URL"
