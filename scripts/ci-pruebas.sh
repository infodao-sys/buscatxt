#!/usr/bin/env bash
# Pruebas basicas del ejecutable compilado. Las usa la integracion continua
# (GitHub Actions) en Linux y en Windows (con Git Bash), y tambien se pueden
# ejecutar a mano:
#
#   cargo build --release
#   ./scripts/ci-pruebas.sh [ruta-al-ejecutable]
#
# Comprueba la busqueda en archivos de texto (recursiva, comodines,
# extensiones, mayusculas), la seguridad de la salida (secuencias de escape),
# los codigos de salida y la lectura de todos los formatos de documento con
# los archivos de ejemplo de tests/fixtures.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
B="${1:-$ROOT/target/release/buscatxt}"
F="$ROOT/tests/fixtures"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
PASSED=0

fail() {
    echo "FALLO: $*" >&2
    exit 1
}

# check DESCRIPCION TEXTO_ESPERADO ARGUMENTOS...
check() {
    local description="$1" expected="$2" output
    shift 2
    output="$("$B" "$@" 2>&1)" || true
    grep -qF -- "$expected" <<<"$output" || fail "$description: se esperaba '$expected' en: $output"
    echo "ok: $description"
    PASSED=$((PASSED + 1))
}

# exit_code DESCRIPCION CODIGO_ESPERADO ARGUMENTOS...
exit_code() {
    local description="$1" expected="$2" code=0
    shift 2
    "$B" "$@" >/dev/null 2>&1 || code=$?
    [[ "$code" == "$expected" ]] || fail "$description: codigo $code, se esperaba $expected"
    echo "ok: $description"
    PASSED=$((PASSED + 1))
}

mkdir -p "$T/sub/profunda"
printf 'hola mundo\nerror en disco duro\nfactura 12025\nprecio: 10*2\n' >"$T/a.txt"
printf 'linea con AGUJA profunda\n' >"$T/sub/profunda/b.txt"
printf 'AGUJA en log\n' >"$T/sub/c.log"
printf 'peligro \033]0;titulo\007 AGUJA\n' >"$T/escape.txt"

echo "== Archivos de texto"
check "version" "buscatxt" -V
check "busqueda recursiva" "AGUJA profunda" AGUJA "$T"
check "comodin *" "error en disco duro" 'error*disco' "$T"
check "comodin ?" "factura 12025" 'factura ?2025' "$T"
check "asterisco literal" "precio: 10*2" '10\*2' "$T"
check "sin distinguir mayusculas" "hola mundo" HOLA "$T" -i
check "otras extensiones" "AGUJA en log" AGUJA "$T" -e log
check "secuencias de escape neutralizadas" '\x{1B}]0;titulo' AGUJA "$T/escape.txt"
output="$("$B" AGUJA "$T/escape.txt")"
[[ "$output" != *$'\033'* ]] || fail "la salida contiene un caracter ESC sin neutralizar"

echo "== Codigos de salida"
exit_code "con coincidencias" 0 AGUJA "$T"
exit_code "sin coincidencias" 1 zzzzzz "$T"
exit_code "opcion desconocida" 2 -x
exit_code "comodin sin comillas (demasiados argumentos)" 2 texto uno dos
exit_code "ruta inexistente" 2 AGUJA "$T/no-existe"

echo "== Documentos"
for file in informe.docx informe.odt informe.pdf informe.doc; do
    check "documento $file" "AGUJA escondida" AGUJA "$F/$file"
done
for file in ventas.xlsx ventas.ods ventas.xls; do
    check "hoja de calculo $file" "2025-05-20" AGUJA "$F/$file"
done
for file in presentacion.pptx presentacion.odp; do
    check "presentacion $file" "diapositiva 2" AGUJA "$F/$file"
done
check "acentos y mayusculas en documentos" "José Pérez" 'josé pérez' "$F" -d -i
count="$("$B" AGUJA "$F" -d | wc -l)"
[[ "$count" -eq 9 ]] || fail "con -d se esperaban 9 resultados y hubo $count"
echo "ok: -d encuentra el texto en los 9 documentos"
PASSED=$((PASSED + 1))

echo
echo "Todas las pruebas pasaron ($PASSED)."
