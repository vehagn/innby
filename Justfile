set dotenv-load

musl_target := "x86_64-unknown-linux-musl"
target_dir := env("CARGO_TARGET_DIR", justfile_directory() / "target")
image := env("FEST_IMAGE", "ghcr.io/vehagn/fest")

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

# Hash an admin password for FEST_ADMIN_PASSWORD_HASH (prompts without echo)
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

# Build the static x86_64 musl release binary (from .sqlx/, no database needed)
build:
    SQLX_OFFLINE=true cargo zigbuild --locked --release --target {{ musl_target }}
    @echo "{{ target_dir }}/{{ musl_target }}/release/fest"

# Build a scratch image that contains only the binary (needs buildah; not in the devcontainer)
image tag="dev": build
    #!/usr/bin/env bash
    set -euo pipefail
    ctr=$(buildah from --arch amd64 --os linux scratch)
    buildah copy --chmod 0555 "$ctr" "{{ target_dir }}/{{ musl_target }}/release/fest" /fest
    buildah config \
      --entrypoint '["/fest"]' \
      --cmd serve \
      --user 65532:65532 \
      --port 8080 \
      --label org.opencontainers.image.source=https://github.com/vehagn/fest \
      --label org.opencontainers.image.version={{ tag }} \
      "$ctr"
    buildah commit --quiet --rm "$ctr" "{{ image }}:{{ tag }}" >/dev/null
    echo "{{ image }}:{{ tag }}"

# Push the image to the registry (run `buildah login ghcr.io` first)
push tag="dev":
    buildah push "{{ image }}:{{ tag }}"

# Open psql against the dev database
psql:
    psql "$DATABASE_URL"
