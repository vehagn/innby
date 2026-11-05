set dotenv-load

target_dir := env("CARGO_TARGET_DIR", justfile_directory() / "target")
image := env("INNBY_IMAGE", "ghcr.io/vehagn/innby")
revision := `git rev-parse HEAD 2>/dev/null || echo unknown`
# Where `just image` writes the OCI image layout; inside the workspace, so the host sees it.
oci_dir := justfile_directory() / "target/oci"
# The container engine for `just up` and `just dev`, e.g. ENGINE=podman. Any Compose
# implementation works, e.g. COMPOSE=docker-compose.
engine := env("ENGINE", "docker")
compose := env("COMPOSE", engine + " compose")
dev_compose := compose + " --file .devcontainer/compose.yaml --file .devcontainer/compose.ports.yaml"

[private]
default:
    @just --list

# Starts the dev container with Postgres, Mailpit and your Vim settings, and opens a
# shell in it. `just dev down` stops it, keeping its database and build cache.
[doc("Open a shell in the dev container; `just dev down` stops it")]
[group("dev")]
dev command="up":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ command }}" in
      up) ;;
      down) exec {{ dev_compose }} down ;;
      *) echo "just dev [up|down]" >&2; exit 1 ;;
    esac
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

# Site :8080, admin :8081, games :8082, mail :8025. In the dev container it serves this
# checkout, with settings from .env and, for the rest, .env.example; on the host it runs
# the published image with compose.yaml. `just up v1.0.0` runs a given tag, or the one
# `just image` wrote, and `just up janitor` applies the local copy's retention rules once.
[doc("Serve this checkout, or on the host run the published image")]
[group("dev")]
up tag="latest":
    #!/usr/bin/env bash
    set -euo pipefail
    urls="http://localhost:8080  http://localhost:8081  http://localhost:8025"
    if [[ -n "${INNBY_DEVCONTAINER:-}" ]]; then
      urls="http://localhost:8080  http://localhost:8081  http://localhost:8082  http://localhost:8025"
      # In the dev container, which has no container engine: serve this checkout instead,
      # on the ports `just dev` publishes. Ctrl-C stops it.
      echo "$urls"
      exec just run
    fi
    if [[ "{{ tag }}" == janitor ]]; then
      exec {{ compose }} run --rm janitor
    fi
    # The tag `just image` last wrote in the dev container is loaded from there.
    built=$(sed -n 's/.*"org.opencontainers.image.ref.name":"\([^"]*\)".*/\1/p' \
      "{{ oci_dir }}/index.json" 2>/dev/null || true)
    if [[ "$built" == "{{ tag }}" ]]; then
      {{ engine }} load --input "{{ oci_dir }}.tar"
    fi
    INNBY_TAG={{ tag }} {{ compose }} up --detach --pull missing
    echo "$urls"

# `down --volumes` deletes the database too.
[doc("Stop the local copy, keeping its database")]
[group("dev")]
down:
    {{ compose }} down

# Database tests get a throwaway database each.
[doc("Run all tests")]
[group("dev")]
test: migrate games
    cargo test --locked
    for manifest in games/*/Cargo.toml; do cargo test --quiet --locked --manifest-path "$manifest"; done
    node --test games/

[doc("Check formatting, lints, the offline query data and dependencies")]
[group("dev")]
lint:
    cargo fmt --check
    cargo clippy --locked --all-targets -- -D warnings
    for manifest in games/*/Cargo.toml; do \
      cargo fmt --check --manifest-path "$manifest" && \
      cargo clippy --quiet --locked --all-targets --manifest-path "$manifest" -- -D warnings; \
    done
    cargo sqlx prepare --check -- --all-targets
    cargo deny check

# Lints the chart and renders it with each values file in charts/innby/ci, checking the
# manifests against the Kubernetes schemas (and the Gateway API's, from the CRDs
# catalog). Settings that can't work must stop the render.
# Benchmarks the static musl release binary the images ship, built for this machine, or
# the one given: every page under load against an event of 1500 guests, failing if a
# page answers wrongly or a process passes its memory budget. See bench/run.sh; CI runs
# it on every release.
[doc("Benchmark a release build; results in target/bench/results.md")]
[group("dev")]
bench binary="":
    #!/usr/bin/env bash
    set -euo pipefail
    binary="{{ binary }}"
    if [[ -z "$binary" ]]; then
      target="$(uname -m)-unknown-linux-musl"
      SQLX_OFFLINE=true cargo zigbuild --locked --release --target "$target"
      binary="{{ target_dir }}/$target/release/innby"
    fi
    bench/run.sh "$binary"

[doc("Lint and render the Helm chart, and check its manifests")]
[group("dev")]
chart:
    #!/usr/bin/env bash
    set -euo pipefail
    chart=charts/innby
    for values in "$chart"/ci/*.yaml; do
      helm lint --strict --quiet "$chart" --values "$values"
      helm template innby "$chart" --values "$values" \
        | kubeconform -strict -summary -kubernetes-version 1.35.0 \
            -schema-location default \
            -schema-location 'https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{{{.Group}}/{{{{.ResourceKind}}_{{{{.ResourceAPIVersion}}.json'
    done
    fails() {
      if helm template innby "$chart" --values "$chart/ci/minimal-values.yaml" "$@" >/dev/null 2>&1; then
        echo "expected the chart to refuse: $*" >&2
        exit 1
      fi
    }
    fails --set siteUrl=fest.example.com
    fails --set admin.url=https://example.com/?admin
    fails --set secrets.adminPasswordHash=
    fails --set secrets.linkKey=
    fails --set database.url=
    fails --set mailer.smtpUrl=
    fails --set httpRoute.enabled=true
    fails --set oidc.issuer=https://auth.example.com
    fails --set extraConfig.HOME=/tmp
    fails --set adminn.enabled=false
    fails --set ciliumNetworkPolicy.enabled=true --set ciliumNetworkPolicy.ingressFromEntities=null
    fails --set database.cnpg.cluster=innby-postgres
    fails --set ciliumNetworkPolicy.enabled=true --set ciliumNetworkPolicy.cnpg.enabled=true
    fails --set database.cnpg.cluster=innby-postgres --set database.existingSecret=db
    fails --set games.internal.enabled=true --set networkPolicy.enabled=true
    fails --set games.internal.enabled=true --set ciliumNetworkPolicy.enabled=true \
      --set ciliumNetworkPolicy.ingressFromEntities[0]=ingress
    echo "The chart refuses the settings that can't work."

# `just db prepare` regenerates the offline query data in .sqlx/, which is committed.
# `just db reset` drops the dev database and builds it again from the migrations.
# `just db seed` loads the sample in seed/sample.sql, replacing an earlier copy of it and
# leaving everything else be; `just db fresh` is a reset and a seed.
[doc("Open psql on the dev database; `just db prepare` updates .sqlx/, `reset` rebuilds it, `seed` loads sample data, `fresh` does both")]
[group("dev")]
db command="psql":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{ command }}" in
      psql) sqlx migrate run; exec psql "$DATABASE_URL" ;;
      prepare) sqlx migrate run; exec cargo sqlx prepare -- --all-targets ;;
      reset) exec sqlx database reset -y ;;
      seed) sqlx migrate run; exec psql "$DATABASE_URL" -v ON_ERROR_STOP=1 --single-transaction -q -f seed/sample.sql ;;
      fresh) sqlx database reset -y; exec psql "$DATABASE_URL" -v ON_ERROR_STOP=1 --single-transaction -q -f seed/sample.sql ;;
      *) echo "just db [psql|prepare|reset|seed|fresh]" >&2; exit 1 ;;
    esac

# E.g. `just innby invite "Kari & Ola" 2`, `just innby janitor`, or `just innby
# hash-password`, which asks for the password without echoing it.
[doc("Run an innby command against the dev database")]
[group("dev")]
[positional-arguments]
innby *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "${1:-}" == hash-password ]]; then
      read -rsp "Password: " password
      echo >&2
      printf %s "$password" | cargo run --quiet -- hash-password
      exit
    fi
    sqlx migrate run
    exec cargo run --quiet -- "$@"

# Bumps Cargo.toml and the chart if needed, tags v<version> (signed) and pushes, which
# builds and publishes the image and the chart.
[doc("Release a version, e.g. `just release 0.3.0`")]
[group("release")]
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
      # The chart is released with the app, under the same version.
      perl -pi -e 's/^version: .*/version: '"$version"'/; s/^appVersion: .*/appVersion: "'"$version"'"/' charts/innby/Chart.yaml
      git commit --quiet --message "Release $tag" -- Cargo.toml Cargo.lock charts/innby/Chart.yaml
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

# Writes the amd64 and arm64 images as one OCI image index to target/oci, as plain files
# with no build tool. `just up <tag>` on the host loads and runs it.
[doc("Build the images into target/oci")]
[group("release")]
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
    echo "{{ oci_dir }}.tar: \`just up {{ tag }}\` on the host loads and runs it"

[doc("Seal deploy/secrets.env with the cluster's key into deploy/sealed-secrets.yaml")]
[group("release")]
seal:
    kubectl create secret generic innby --namespace innby \
      --from-env-file=deploy/secrets.env --dry-run=client --output=yaml \
      | kubeseal --controller-namespace sealed-secrets \
          --controller-name sealed-secrets-controller --format yaml \
      > deploy/sealed-secrets.yaml
    @echo "deploy/sealed-secrets.yaml: add it to deploy/kustomization.yaml"

# Run the site (:8080), the admin UI (:8081), the mailer and the example games (:8082) from this checkout (reads .env, then .env.example)
[private]
run: migrate games
    #!/usr/bin/env bash
    set -euo pipefail
    # What neither your shell nor .env sets comes from .env.example, as with Compose on
    # the host, so a new setting works before your .env has it.
    while IFS= read -r -d '' setting; do
      name=${setting%%=*}
      [[ -n "${!name+set}" ]] || export "$setting"
    done < <(env -i bash -c 'set -a; . ./.env.example; env -0')
    # A database with no events gets the sample, and with it the event the helper user
    # in .env.example may see.
    if [[ "$(psql "$DATABASE_URL" -tAc 'select count(*) from event')" == 0 ]]; then
      just db seed
    fi
    cargo build --quiet
    trap 'kill 0' EXIT
    cargo run --quiet -- admin &
    cargo run --quiet -- mailer &
    # The example games, on an origin of their own, as docs/games.md says they must be,
    # each game with a server given its key.
    for game in $(tr ';' '\n' <<< "${INNBY_GAMES:-}" | grep ':server:' | cut -d: -f1 | tr -d ' '); do
      export "INNBY_GAME_KEY_$(tr 'a-z-' 'A-Z_' <<< "$game")=$(cargo run --quiet -- game-key "$game")"
    done
    INNBY_URL="${INNBY_BASE_URL:-}" node games/serve.js &
    cargo run --quiet -- serve

# Builds the example games written in Rust (games/*/Cargo.toml) to WebAssembly, beside
# their pages, where games/serve.js finds them
[private]
games:
    #!/usr/bin/env bash
    set -euo pipefail
    for manifest in games/*/Cargo.toml; do
      dir=$(dirname "$manifest")
      name=$(basename "$dir")
      cargo build --quiet --locked --release --target wasm32-unknown-unknown --manifest-path "$manifest"
      cp "{{ target_dir }}/wasm32-unknown-unknown/release/$name.wasm" "$dir/$name.wasm"
    done

# Apply the migrations to $DATABASE_URL (the query macros check against it at compile time)
[private]
migrate:
    sqlx migrate run

# Build static musl release binaries for amd64 and arm64 (offline, from .sqlx/)
[private]
build:
    SQLX_OFFLINE=true cargo zigbuild --locked --release \
      --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl

# Push the image index from `just image` under each tag given (`skopeo login ghcr.io` first); CI does this
[private]
push tag="dev" *also:
    #!/usr/bin/env bash
    set -euo pipefail
    for tag in {{ tag }} {{ also }}; do
      skopeo copy --all --preserve-digests "oci:{{ oci_dir }}:{{ tag }}" "docker://{{ image }}:$tag"
    done
