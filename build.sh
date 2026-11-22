#!/usr/bin/env sh
# Compilation, via Docker si disponible, sinon avec la chaîne locale (rustup + zig + cargo-zigbuild).
#   ./build.sh            -> dist/heic-viewer.exe (Windows)
#   ./build.sh linux      -> dist/heic-viewer (Linux)
#   ./build.sh test       -> tests (dont le suivi réel d'un dossier temporaire)
#   ./build.sh cargo ...  -> commande cargo arbitraire (ex: ./build.sh cargo clippy)
set -eu
cd "$(dirname "$0")"

if command -v docker >/dev/null 2>&1; then
    IMAGE=heic-viewer-build
    docker build -q -t "$IMAGE" . >/dev/null
    # Cache cargo (registry) partagé entre les builds, dans un volume Docker.
    docker volume create heic-viewer-cargo >/dev/null
    # Le volume est créé par root : on le donne à l'utilisateur courant.
    docker run --rm -v heic-viewer-cargo:/cargo "$IMAGE" chown "$(id -u):$(id -g)" /cargo
    run() {
        docker run --rm \
            -u "$(id -u):$(id -g)" \
            -e CARGO_HOME=/cargo -e HEIC_SAMPLE \
            -v heic-viewer-cargo:/cargo \
            -v "$PWD":/src \
            "$IMAGE" "$@"
    }
    windows_build() { run cargo build --release --target x86_64-pc-windows-gnu; }
    TARGET_DIR=target
else
    # Pas de Docker : zig sert d'éditeur de liens (voir README).
    run() { "$@"; }
    windows_build() { cargo zigbuild --release --target x86_64-pc-windows-gnu; }
    TARGET_DIR=${CARGO_TARGET_DIR:-target}
fi

mkdir -p dist
case "${1:-windows}" in
    windows)
        windows_build
        cp "$TARGET_DIR/x86_64-pc-windows-gnu/release/heic-viewer.exe" dist/
        echo "OK -> dist/heic-viewer.exe"
        ;;
    linux)
        run cargo build --release
        cp "$TARGET_DIR/release/heic-viewer" dist/
        echo "OK -> dist/heic-viewer"
        ;;
    test)
        # Le test de décodage d'une vraie photo ne tourne que si HEIC_SAMPLE est défini.
        run cargo test --release -- ${HEIC_SAMPLE:+--include-ignored}
        ;;
    cargo)
        shift
        run cargo "$@"
        ;;
    *)
        echo "usage: $0 [windows|linux|test|cargo ...]" >&2
        exit 1
        ;;
esac
