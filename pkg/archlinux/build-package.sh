#!/usr/bin/env bash
#
# build-package.sh — Construye el paquete Arch Linux (.pkg.tar.zst) de FamilyCom
#
# Uso:
#   ./pkg/archlinux/build-package.sh          # construir paquete
#   ./pkg/archlinux/build-package.sh --install # construir e instalar
#
# Este script:
#   1. Crea un tarball del codigo fuente (excluyendo target/, .git/, pkg/)
#   2. Genera el checksum SHA256 en el PKGBUILD
#   3. Ejecuta makepkg para construir el paquete
#   4. Muestra instrucciones para instalar

set -euo pipefail

# --- Resolve project root (where Cargo.toml lives) ---
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
PKG_DIR="$SCRIPT_DIR"

# Metadata (must match PKGBUILD)
PKGNAME="familycom"
PKGVER="0.1.0"
TARBALL="${PKGNAME}-${PKGVER}.tar.gz"

# --- Verify we're in the right place ---
if [[ ! -f "$PROJECT_ROOT/Cargo.toml" ]]; then
    echo "Error: No se encontro Cargo.toml en $PROJECT_ROOT" >&2
    echo "Ejecuta este script desde la raiz del proyecto FamilyCom." >&2
    exit 1
fi

# --- Check for Cargo.lock (needed for --frozen builds) ---
if [[ ! -f "$PROJECT_ROOT/Cargo.lock" ]]; then
    echo "Generando Cargo.lock..."
    (cd "$PROJECT_ROOT" && cargo generate-lockfile)
fi

echo "==> Creando tarball del codigo fuente..."

# Create the tarball from project root, placing files under familycom-0.1.0/
# Exclude build artifacts, git directory, and packaging directory
tar -czf "$PKG_DIR/$TARBALL" \
    --transform "s,^.,${PKGNAME}-${PKGVER}," \
    --exclude='./target' \
    --exclude='./.git' \
    --exclude='./pkg' \
    -C "$PROJECT_ROOT" .

echo "    Tarball: $PKG_DIR/$TARBALL"

# --- Update checksum in PKGBUILD ---
echo "==> Calculando checksum SHA256..."
CHECKSUM=$(sha256sum "$PKG_DIR/$TARBALL" | cut -d' ' -f1)
sed -i "s/^sha256sums=.*/sha256sums=('$CHECKSUM')/" "$PKG_DIR/PKGBUILD"
echo "    SHA256: $CHECKSUM"

# --- Build the package ---
echo "==> Construyendo paquete con makepkg..."
echo ""

# Run makepkg from the packaging directory
# -s: install missing dependencies (asks for sudo)
# -f: force rebuild even if package already exists
(cd "$PKG_DIR" && makepkg -sf)

# --- Find the built package ---
PKG_FILE=$(ls -t "$PKG_DIR"/${PKGNAME}-${PKGVER}-*.pkg.tar.zst 2>/dev/null | head -1)

if [[ -z "$PKG_FILE" ]]; then
    echo "Error: No se encontro el paquete construido." >&2
    exit 1
fi

echo ""
echo "=========================================="
echo "  Paquete construido exitosamente!"
echo "=========================================="
echo ""
echo "  Archivo: $PKG_FILE"
echo ""
echo "  Para instalar en esta maquina:"
echo "    sudo pacman -U $PKG_FILE"
echo ""
echo "  Para instalar en otra maquina (copiar primero):"
echo "    scp $PKG_FILE usuario@otra-maquina:~/"
echo "    ssh usuario@otra-maquina sudo pacman -U ~/$(basename "$PKG_FILE")"
echo ""

# --- Optional: install immediately ---
if [[ "${1:-}" == "--install" ]]; then
    echo "==> Instalando paquete..."
    sudo pacman -U "$PKG_FILE"
fi
