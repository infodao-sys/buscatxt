#!/usr/bin/env bash
# Instala (o desinstala) el comando "buscatxt" en Linux.
#
# Uso:
#   ./scripts/instalar-linux.sh                # solo para tu usuario (~/.local/bin), sin sudo
#   ./scripts/instalar-linux.sh --sistema      # para todos los usuarios (/usr/local/bin), pide sudo
#   ./scripts/instalar-linux.sh --desinstalar  # quita el comando (de ambos lugares)
#
# El script no descarga nada: copia el ejecutable incluido en dist/ (o el
# compilado con "cargo build --release" si no existe).

set -euo pipefail

NAME="buscatxt"
MARKER="# agregado por buscatxt"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
USER_DIR="$HOME/.local/bin"
SYSTEM_DIR="/usr/local/bin"

usage() {
    sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

# Archivos de inicio de la terminal donde se agrega ~/.local/bin al PATH.
shell_files() {
    local files=("$HOME/.bashrc")
    [[ -f "$HOME/.zshrc" ]] && files+=("$HOME/.zshrc")
    printf '%s\n' "${files[@]}"
}

find_binary() {
    local candidates=("$ROOT/dist/buscatxt-linux-x86_64" "$ROOT/target/release/buscatxt")
    if [[ "$(uname -m)" != "x86_64" ]]; then
        # El binario de dist/ es para x86_64: en otras arquitecturas hay que compilar.
        candidates=("$ROOT/target/release/buscatxt")
    fi
    for candidate in "${candidates[@]}"; do
        if [[ -f "$candidate" ]] && "$candidate" -V >/dev/null 2>&1; then
            echo "$candidate"
            return 0
        fi
    done
    echo "error: no se encontro un ejecutable de $NAME que funcione en este equipo." >&2
    echo "Compilalo primero con:  cargo build --release" >&2
    return 1
}

install_command() {
    local dir="$1" use_sudo="$2" source
    source="$(find_binary)"

    if [[ "$use_sudo" == "si" ]]; then
        sudo install -d "$dir"
        sudo install -m 755 "$source" "$dir/$NAME"
    else
        install -d "$dir"
        install -m 755 "$source" "$dir/$NAME"
    fi
    echo "Instalado: $dir/$NAME ($("$dir/$NAME" -V))"

    # Si la carpeta no esta en el PATH, se agrega en los archivos de inicio
    # de la terminal (una sola vez, marcada para poder quitarla).
    case ":$PATH:" in
        *":$dir:"*) ;;
        *)
            if [[ "$dir" == "$USER_DIR" ]]; then
                while read -r file; do
                    if ! grep -qF "$MARKER" "$file" 2>/dev/null; then
                        # Salto de linea previo solo si el archivo no termina en uno.
                        if [[ -s "$file" && "$(tail -c 1 "$file")" != "" ]]; then
                            echo >>"$file"
                        fi
                        printf 'export PATH="$HOME/.local/bin:$PATH"  %s\n' "$MARKER" >>"$file"
                        echo "Se agrego ~/.local/bin al PATH en $file"
                    fi
                done < <(shell_files)
                echo
                echo "Abre una terminal nueva (o ejecuta: source ~/.bashrc) para usar el comando."
            fi
            ;;
    esac

    echo
    echo "Prueba:  $NAME --help"
}

uninstall_command() {
    local removed=0
    if [[ -f "$USER_DIR/$NAME" ]]; then
        rm -f "$USER_DIR/$NAME"
        echo "Eliminado: $USER_DIR/$NAME"
        removed=1
    fi
    if [[ -f "$SYSTEM_DIR/$NAME" ]]; then
        sudo rm -f "$SYSTEM_DIR/$NAME"
        echo "Eliminado: $SYSTEM_DIR/$NAME"
        removed=1
    fi
    while read -r file; do
        if grep -qF "$MARKER" "$file" 2>/dev/null; then
            # Solo se quita la linea que agrego este script.
            sed -i "/$MARKER\$/d" "$file"
            echo "Se quito la linea agregada en $file"
        fi
    done < <(shell_files)
    if [[ "$removed" == 0 ]]; then
        echo "$NAME no estaba instalado."
    fi
}

case "${1:-}" in
    "") install_command "$USER_DIR" no ;;
    --sistema) install_command "$SYSTEM_DIR" si ;;
    --desinstalar) uninstall_command ;;
    -h | --help) usage ;;
    *)
        echo "opcion desconocida: $1" >&2
        usage >&2
        exit 2
        ;;
esac
