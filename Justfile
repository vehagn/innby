set dotenv-load

target_dir := env("CARGO_TARGET_DIR", justfile_directory() / "target")
image := env("INNBY_IMAGE", "ghcr.io/vehagn/innby")
revision := `git rev-parse HEAD 2>/dev/null || echo unknown`
# Compose for `just up`; any Compose implementation works, e.g. COMPOSE="podman compose".
compose := env("COMPOSE", "docker compose")

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

# Build an amd64 and an arm64 scratch image into one image index (needs buildah)
image tag="dev": build
    #!/usr/bin/env bash
    set -euo pipefail
    index="{{ image }}:{{ tag }}"
    buildah manifest rm "$index" >/dev/null 2>&1 || true
    buildah manifest create "$index" >/dev/null
    for arch in amd64 arm64; do
      case "$arch" in
        amd64) target=x86_64-unknown-linux-musl ;;
        arm64) target=aarch64-unknown-linux-musl ;;
      esac
      ctr=$(buildah from scratch)
      buildah copy --chmod 0555 "$ctr" "{{ target_dir }}/$target/release/innby" /innby
      buildah config \
        --os linux \
        --arch "$arch" \
        --entrypoint '["/innby"]' \
        --cmd serve \
        --user 65532:65532 \
        --port 8080 \
        --label org.opencontainers.image.source=https://github.com/vehagn/innby \
        --label org.opencontainers.image.version={{ tag }} \
        --label org.opencontainers.image.revision={{ revision }} \
        "$ctr"
      buildah commit --quiet --rm --format oci --manifest "$index" "$ctr" >/dev/null
    done
    echo "$index"

# Push the OCI image index under each tag given (`buildah login ghcr.io` first)
push tag="dev" *also:
    #!/usr/bin/env bash
    set -euo pipefail
    for tag in {{ tag }} {{ also }}; do
      buildah manifest push --all --format oci "{{ image }}:{{ tag }}" "docker://{{ image }}:$tag"
    done

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

# Run a local copy of the published image (see compose.yaml): site :8080, admin :8081, mail :8025
up tag="latest":
    INNBY_TAG={{ tag }} {{ compose }} up --detach --pull missing
    @echo "http://localhost:8080  http://localhost:8081  http://localhost:8025"

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
