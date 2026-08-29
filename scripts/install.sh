#!/bin/sh
# Build y instalación local de `zek`.
#
# Uso:
#   ./scripts/install.sh                 # instala en ~/.local/bin
#   PREFIX=/usr/local ./scripts/install.sh
#   PREFIX=$HOME/bin ./scripts/install.sh
#
# Requiere: cargo (Rust 1.80+).

set -eu

ROOT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

cd "$ROOT_DIR"

echo "==> Compilando zek en modo release..."
cargo build --release

PREFIX=${PREFIX:-"$HOME/.local/bin"}

if [ ! -d "$PREFIX" ]; then
    echo "==> Creando $PREFIX"
    mkdir -p "$PREFIX"
fi

echo "==> Instalando zek en $PREFIX"
cp "target/release/zek" "$PREFIX/zek"
chmod +x "$PREFIX/zek"

echo
echo "zek instalado en $PREFIX/zek"
case ":$PATH:" in
    *":$PREFIX:"*) ;;
    *) echo "Aviso: $PREFIX no está en tu PATH." ;;
esac
