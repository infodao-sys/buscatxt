//! # buscatxt
//!
//! Herramienta de consola que busca un texto (con comodines `*` y `?`)
//! dentro de archivos, recorriendo carpetas de forma recursiva. Funciona en
//! Linux y Windows y solo usa la biblioteca estandar de Rust.
//!
//! ## Flujo general
//!
//! 1. `parse_args` lee la linea de comandos y arma un [`Config`].
//! 2. `Pattern::new` convierte el texto buscado en una lista de segmentos
//!    (los trozos separados por `*`).
//! 3. `run` decide si la ruta es un archivo o una carpeta:
//!    - archivo: se busca directamente con `search_file`;
//!    - carpeta: un hilo (`walk`) recorre las carpetas y reparte los archivos
//!      a varios hilos de trabajo, que llaman a `search_file`. El hilo
//!      principal recibe los resultados y los imprime en el orden del
//!      recorrido, asi la salida es siempre la misma.
//! 4. `search_file` lee cada archivo linea por linea (sin cargarlo entero),
//!    detecta su codificacion y busca el patron en cada linea. Las lineas
//!    gigantes (mas de `MAX_LINE_BYTES`) se procesan por bloques en
//!    `search_long_line`.
//! 5. Los documentos (DOCX, XLSX, PPTX, DOC, XLS, PPT, ODT, ODS, ODP y PDF) se reconocen por
//!    su extension: el modulo `docs` extrae su texto en "unidades" (parrafo,
//!    fila de hoja, linea de pagina...) y `search_document` busca en ellas.
//!
//! ## Seguridad
//!
//! - El programa solo LEE archivos: no ejecuta comandos, no carga librerias
//!   y no abre conexiones de red. No hay codigo `unsafe`.
//! - Todo texto que sale de un archivo (contenido o nombre) pasa por
//!   `sanitize`, que neutraliza secuencias de escape de la terminal.
//! - No se siguen enlaces simbolicos ni uniones de Windows, para evitar
//!   bucles infinitos y salir de la carpeta indicada.
//! - La memoria usada esta acotada aunque los archivos o sus lineas sean
//!   enormes.
//!
//! ## Codigos de salida
//!
//! `0` = hubo coincidencias, `1` = no hubo, `2` = error de uso o de acceso.

// Ninguna parte del programa puede usar codigo `unsafe`: el compilador lo
// rechaza (tambien en los modulos de `docs`).
#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Mutex;
use std::thread;

/// Extraccion de texto de documentos (DOCX, XLSX, PPTX, DOC, XLS, PPT, ODT,
/// ODS, ODP, PDF).
mod docs;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Escribe un aviso en stderr. A diferencia de `eprintln!`, no provoca un
/// panico si stderr esta cerrado (por ejemplo con `2>&1 | head`).
macro_rules! warn {
    ($($arg:tt)*) => {{
        let _ = writeln!(io::stderr(), $($arg)*);
    }};
}

/// Escribe en stdout sin provocar un panico si la salida se cerro
/// (por ejemplo, `buscatxt --help | head -1`).
macro_rules! say {
    ($($arg:tt)*) => {{
        let _ = writeln!(io::stdout(), $($arg)*);
    }};
}

/// Indica si un error de escritura se debe a que el programa que recibe la
/// salida ya termino (`| head`, `| more` cerrado...). No es un fallo real:
/// simplemente se deja de escribir. En Windows llega con distintos codigos
/// segun la version: 109 (ERROR_BROKEN_PIPE), 232 (ERROR_NO_DATA) o
/// 233 (ERROR_PIPE_NOT_CONNECTED).
fn is_broken_pipe(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::BrokenPipe
        || (cfg!(windows) && matches!(error.raw_os_error(), Some(109 | 232 | 233)))
}

/// Tamano maximo que se guarda en memoria de una sola linea. Las lineas mas
/// largas se procesan por bloques de este tamano.
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Caracteres de contexto que se muestran a cada lado de la coincidencia
/// cuando la linea supera `MAX_LINE_BYTES`.
const CONTEXT_CHARS: usize = 100;

/// Bytes iniciales que se revisan para decidir si un archivo es binario.
const BINARY_PROBE: usize = 8192;

// ---------------------------------------------------------------------------
// Configuracion y argumentos
// ---------------------------------------------------------------------------

/// Opciones elegidas por el usuario en la linea de comandos.
struct Config {
    /// Texto a buscar tal como lo escribio el usuario (con comodines).
    pattern: String,
    /// Carpeta o archivo donde buscar, convertido a ruta absoluta.
    root: PathBuf,
    /// `-i`: ignorar mayusculas y minusculas.
    ignore_case: bool,
    /// `-l`: mostrar solo la ruta de los archivos con coincidencias.
    files_only: bool,
    /// `false` con `-F`: `*` y `?` se buscan como caracteres normales.
    wildcards: bool,
    /// Extensiones permitidas, en minusculas y sin punto.
    /// `None` significa "todas las extensiones" (`-a`).
    extensions: Option<Vec<String>>,
    /// Salida a una terminal: se usan colores y encabezados por archivo.
    /// Si la salida va a un archivo o a otro programa, se usa el formato
    /// clasico `ruta:linea:contenido`.
    color: bool,
    /// Cantidad de hilos que leen archivos en paralelo.
    threads: usize,
}

impl Config {
    /// Indica si el archivo tiene una de las extensiones pedidas.
    /// La comparacion no distingue mayusculas (`.TXT` == `.txt`).
    fn matches_extension(&self, path: &Path) -> bool {
        match &self.extensions {
            None => true,
            Some(list) => match path.extension() {
                Some(ext) => {
                    let ext = ext.to_string_lossy().to_lowercase();
                    list.contains(&ext)
                }
                None => false,
            },
        }
    }
}

/// Texto de `--help`. Las llaves `{{ }}` y barras `\\` estan duplicadas
/// porque es una cadena con formato.
fn print_help() {
    say!(
        "buscatxt {VERSION}
Busca un texto dentro de archivos de forma recursiva (Linux y Windows).

USO:
    buscatxt <TEXTO> [RUTA] [OPCIONES]

ARGUMENTOS:
    <TEXTO>    Texto a buscar (admite comodines, ver COMODINES)
    [RUTA]     Archivo o carpeta donde buscar (por defecto: .)

COMODINES:
    Comodin   Significado                              Ejemplo        Encuentra
    -------   --------------------------------------   ------------   ------------------
    *         cualquier cantidad de caracteres,        error*disco    error en disco
              incluso ninguno (dentro de la linea)                    errordisco
    ?         exactamente un caracter                  a?o            ano, año, a1o
    \\*        un asterisco literal                     10\\*2          10*2
    \\?        un signo de interrogacion literal        hola\\?         hola?
    \\\\        una barra invertida literal              C:\\\\temp       C:\\temp

    - El texto se busca en cualquier parte de la linea: no hace falta
      poner * al inicio ni al final.
    - Se pueden combinar: \"fact*20??\" encuentra \"factura del 2025\".
    - Una barra que no va antes de *, ? o \\ se toma literal, asi que
      \"C:\\temp\" tambien funciona.
    - Con -F los comodines se desactivan y todo se busca tal cual.

OPCIONES:
    -i, --ignore-case          Ignora mayusculas y minusculas
    -l, --files-with-matches   Muestra solo la ruta de los archivos
    -e, --extension <EXT>      Extensiones a buscar, separadas por comas o
                               repitiendo la opcion (por defecto: txt)
    -d, --documentos           Busca tambien en documentos de Office,
                               LibreOffice y PDF (ver DOCUMENTOS)
    -a, --all                  Busca en todos los archivos
    -F, --fixed-strings        Trata * y ? como texto normal (sin comodines)
    -j, --threads <N>          Cantidad de hilos de busqueda
    -h, --help                 Muestra esta ayuda
    -V, --version              Muestra la version

DOCUMENTOS:
    Con -d se extrae y se busca el texto de estos documentos:
      Word, Excel, PowerPoint   docx, xlsx, pptx (y docm, xlsm, pptm...)
      Office 97-2003            doc, xls, ppt
      LibreOffice               odt, ods, odp
      PDF                       pdf
    Tambien se puede elegir solo algunos: -e docx,pdf. Cada resultado indica
    donde esta el texto: parrafo, hoja y fila, diapositiva o pagina.
    No se puede leer el texto de PDF escaneados (son imagenes), de
    documentos protegidos con contrasena ni de Word 6/95 o Excel 5/95.

IMPORTANTE:
    Escribe siempre el texto entre comillas si lleva espacios, * o ?.
    Sin comillas, la terminal de Linux reemplaza el comodin por nombres
    de archivo. En PowerShell usa comillas simples ('texto'), porque en
    las dobles reemplaza las $variables.

NOTAS:
    Cada resultado muestra la ruta completa del archivo y el numero de
    linea. Las lineas de mas de 1 MiB se procesan por bloques y se muestran
    recortadas alrededor de la coincidencia. Los caracteres de control
    se muestran como \\x{{1B}} para no afectar a la terminal. Los archivos
    binarios solo se informan, sin mostrar su contenido.

EJEMPLOS:
    buscatxt \"hola mundo\"
    buscatxt \"error*disco\" ./registros -i
    buscatxt \"factura ?2025\" . -e txt,csv,log
    buscatxt \"TODO\" C:\\proyectos -e rs -e md -l
    buscatxt \"precio: 10\\*2\" . -a
    buscatxt \"a?o 20??\" . -i
    buscatxt \"contrato*2025\" ~/Documentos -d -i"
    );
}

/// Agrega a `list` las extensiones de un valor de `-e`. Acepta varias
/// separadas por comas y las formas `txt`, `.txt` y `*.txt`. Un `*` solo
/// equivale a `-a` (todas las extensiones).
fn parse_extensions(value: &str, list: &mut Vec<String>, all: &mut bool) {
    for item in value.split(',') {
        let item = item.trim();
        let item = item.strip_prefix("*.").unwrap_or(item);
        let item = item.strip_prefix('.').unwrap_or(item);
        if item == "*" {
            *all = true;
        } else if !item.is_empty() {
            let item = item.to_lowercase();
            if !list.contains(&item) {
                list.push(item);
            }
        }
    }
}

/// Valida el valor de `-j` (entre 1 y 256 hilos).
fn parse_threads(value: &str) -> Result<usize, String> {
    match value.trim().parse::<usize>() {
        Ok(n) if (1..=256).contains(&n) => Ok(n),
        _ => Err(format!("valor invalido para -j/--threads: {value} (usa 1 a 256)")),
    }
}

/// Toma el siguiente argumento como valor de una opcion (por ejemplo, el
/// `txt` de `-e txt`).
fn next_value(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("falta el valor para {option}"))?
        .into_string()
        .map_err(|_| format!("el valor de {option} no es texto valido"))
}

/// Lee los argumentos del programa. Se escribe a mano (sin bibliotecas
/// externas) y admite:
/// - opciones largas: `--extension txt` o `--extension=txt`;
/// - opciones cortas agrupadas: `-il`, y con valor pegado: `-etxt`;
/// - `--` para indicar que lo que sigue no son opciones (para buscar un
///   texto que empieza con `-`).
///
/// Se usa `args_os` en vez de `args` porque `args` aborta el programa si un
/// argumento (por ejemplo, una ruta en Linux) no es UTF-8 valido.
fn parse_args() -> Result<Config, String> {
    let mut ignore_case = false;
    let mut files_only = false;
    let mut wildcards = true;
    let mut all = false;
    let mut extensions: Vec<String> = Vec::new();
    // Si el usuario eligio extensiones con -e, no se agrega `txt` por defecto.
    let mut explicit_extensions = false;
    // Por defecto, un hilo por nucleo (maximo 16).
    let mut threads = thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let mut positional: Vec<OsString> = Vec::new();
    let mut positional_only = false;
    let mut args = env::args_os().skip(1);

    while let Some(os_arg) = args.next() {
        if positional_only {
            positional.push(os_arg);
            continue;
        }
        let Some(arg) = os_arg.to_str() else {
            positional.push(os_arg);
            continue;
        };

        if arg == "--" {
            positional_only = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let takes_value = matches!(name, "extension" | "threads");
            if inline.is_some() && !takes_value {
                return Err(format!("la opcion --{name} no lleva valor"));
            }
            let mut value = || match &inline {
                Some(value) => Ok(value.clone()),
                None => next_value(&mut args, &format!("--{name}")),
            };
            match name {
                "help" => {
                    print_help();
                    process::exit(0);
                }
                "version" => {
                    say!("buscatxt {VERSION}");
                    process::exit(0);
                }
                "ignore-case" => ignore_case = true,
                "files-with-matches" => files_only = true,
                "all" => all = true,
                "documentos" | "documents" => {
                    parse_extensions(&docs::EXTENSIONS.join(","), &mut extensions, &mut all)
                }
                "fixed-strings" => wildcards = false,
                "extension" => {
                    explicit_extensions = true;
                    parse_extensions(&value()?, &mut extensions, &mut all)
                }
                "threads" => threads = parse_threads(&value()?)?,
                _ => return Err(format!("opcion desconocida: --{name}")),
            }
        } else if arg.starts_with('-') && arg.len() > 1 {
            // Opciones cortas, se pueden agrupar: -il, -e txt, -etxt
            let flags: Vec<char> = arg[1..].chars().collect();
            for (index, flag) in flags.iter().enumerate() {
                match flag {
                    'h' => {
                        print_help();
                        process::exit(0);
                    }
                    'V' => {
                        say!("buscatxt {VERSION}");
                        process::exit(0);
                    }
                    'i' => ignore_case = true,
                    'l' => files_only = true,
                    'a' => all = true,
                    'd' => parse_extensions(&docs::EXTENSIONS.join(","), &mut extensions, &mut all),
                    'F' => wildcards = false,
                    'e' | 'j' => {
                        let rest: String = flags[index + 1..].iter().collect();
                        let value = if rest.is_empty() {
                            next_value(&mut args, &format!("-{flag}"))?
                        } else {
                            rest
                        };
                        if *flag == 'e' {
                            explicit_extensions = true;
                            parse_extensions(&value, &mut extensions, &mut all);
                        } else {
                            threads = parse_threads(&value)?;
                        }
                        break;
                    }
                    other => return Err(format!("opcion desconocida: -{other}")),
                }
            }
        } else {
            positional.push(os_arg);
        }
    }

    if positional.is_empty() {
        return Err(String::from("falta el texto a buscar"));
    }
    // Mas de dos argumentos suele significar que el texto se escribio sin
    // comillas: o tenia espacios (`factura 2025` llega como dos argumentos)
    // o la terminal expandio un comodin (`hol*` -> `hola1.txt hola2.txt`;
    // pasa en bash y en PowerShell de Linux/macOS). Se avisa en vez de
    // buscar algo que el usuario no escribio.
    if positional.len() > 2 {
        return Err(format!(
            "demasiados argumentos ({}). Si el texto tiene espacios o comodines \
             (* o ?), escribelo entre comillas, por ejemplo: buscatxt \"factura ?2025\" . \
             (en PowerShell usa comillas simples: 'factura ?2025')",
            positional.len()
        ));
    }

    let mut positional = positional.into_iter();
    let pattern = positional
        .next()
        .unwrap_or_default()
        .into_string()
        .map_err(|_| String::from("el texto a buscar no es texto valido (UTF-8)"))?;
    if pattern.is_empty() {
        return Err(String::from("el texto a buscar no puede estar vacio"));
    }

    let root = positional.next().map_or_else(|| PathBuf::from("."), PathBuf::from);
    // Ruta absoluta para mostrar siempre donde esta cada archivo.
    let root = std::path::absolute(&root).unwrap_or(root);

    // Por defecto se busca en .txt. Con -d (sin -e) se buscan los .txt y
    // ademas los documentos. Con -e solo se buscan las extensiones elegidas.
    if !all && !explicit_extensions {
        extensions.insert(0, String::from("txt"));
    }

    Ok(Config {
        pattern,
        root,
        ignore_case,
        files_only,
        wildcards,
        extensions: if all { None } else { Some(extensions) },
        color: io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
        threads,
    })
}

// ---------------------------------------------------------------------------
// Patron con comodines
// ---------------------------------------------------------------------------

// Como funciona la busqueda con comodines:
//
// El patron se divide por los `*` en segmentos. Por ejemplo
// `error*disco?duro` da los segmentos `error` y `disco?duro`. Una linea
// coincide si contiene los segmentos en orden, de izquierda a derecha; lo
// que haya entre ellos es lo que cubre el `*`. Basta con buscar cada segmento
// en su primera aparicion posible despues del anterior (si existe una
// coincidencia, la busqueda "lo antes posible" tambien la encuentra), asi que
// no hace falta retroceder y el costo es lineal en la mayoria de los casos.
//
// Dentro de un segmento, `?` coincide con exactamente un caracter Unicode.
// Los segmentos sin `?` se buscan con `str::find`, que es muy rapido.

/// Elemento de un segmento: un caracter fijo o el comodin `?`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tok {
    Lit(char),
    Any,
}

/// Trozo del patron entre dos `*`. Puede contener `?`.
struct Segment {
    toks: Vec<Tok>,
    /// Texto fijo cuando el segmento no tiene `?` (busqueda rapida).
    literal: Option<String>,
}

impl Segment {
    fn new(toks: Vec<Tok>) -> Self {
        let literal = if toks.iter().all(|tok| matches!(tok, Tok::Lit(_))) {
            Some(
                toks.iter()
                    .filter_map(|tok| match tok {
                        Tok::Lit(c) => Some(*c),
                        Tok::Any => None,
                    })
                    .collect(),
            )
        } else {
            None
        };
        Segment { toks, literal }
    }

    /// Comprueba si el segmento coincide empezando exactamente en el byte
    /// `start`. Devuelve el byte donde termina la coincidencia.
    fn match_at(&self, hay: &str, start: usize) -> Option<usize> {
        let mut chars = hay[start..].char_indices();
        let mut end = start;
        for tok in &self.toks {
            let (index, c) = chars.next()?;
            if let Tok::Lit(expected) = tok {
                if *expected != c {
                    return None;
                }
            }
            end = start + index + c.len_utf8();
        }
        Some(end)
    }

    /// Primera aparicion del segmento en `hay` a partir del byte `from`.
    fn find(&self, hay: &str, from: usize) -> Option<(usize, usize)> {
        if let Some(literal) = &self.literal {
            return hay[from..]
                .find(literal.as_str())
                .map(|index| (from + index, from + index + literal.len()));
        }
        // Segmento con `?`: se prueba cada posicion candidata. Si empieza con
        // un caracter fijo, se salta directo a sus apariciones.
        let mut pos = from;
        while pos < hay.len() {
            let start = match self.toks[0] {
                Tok::Lit(c) => pos + hay[pos..].find(c)?,
                Tok::Any => pos,
            };
            if let Some(end) = self.match_at(hay, start) {
                return Some((start, end));
            }
            pos = start + hay[start..].chars().next().map_or(1, char::len_utf8);
        }
        None
    }
}

/// Patron de busqueda ya procesado.
struct Pattern {
    /// Segmentos entre `*`. Si esta vacio (el patron era solo `*`), coincide
    /// con cualquier linea.
    segments: Vec<Segment>,
    ignore_case: bool,
    /// Largo en caracteres del segmento mas largo.
    max_chars: usize,
}

impl Pattern {
    /// Convierte el texto del usuario en segmentos. Con `ignore_case`, los
    /// caracteres fijos se guardan ya en minusculas para compararlos con la
    /// linea tambien pasada a minusculas.
    fn new(text: &str, wildcards: bool, ignore_case: bool) -> Self {
        let fold = |c: char| if ignore_case { fold_char(c) } else { c };
        let mut segments = Vec::new();
        let mut current = Vec::new();
        let mut chars = text.chars();

        while let Some(c) = chars.next() {
            if !wildcards {
                current.push(Tok::Lit(fold(c)));
                continue;
            }
            match c {
                '\\' => match chars.next() {
                    Some(next @ ('*' | '?' | '\\')) => current.push(Tok::Lit(fold(next))),
                    Some(next) => {
                        // No es un escape: se conserva la barra (rutas de Windows).
                        current.push(Tok::Lit('\\'));
                        current.push(Tok::Lit(fold(next)));
                    }
                    None => current.push(Tok::Lit('\\')),
                },
                '*' => {
                    if !current.is_empty() {
                        segments.push(Segment::new(std::mem::take(&mut current)));
                    }
                }
                '?' => current.push(Tok::Any),
                _ => current.push(Tok::Lit(fold(c))),
            }
        }
        if !current.is_empty() {
            segments.push(Segment::new(current));
        }

        let max_chars = segments.iter().map(|s| s.toks.len()).max().unwrap_or(0);
        Pattern {
            segments,
            ignore_case,
            max_chars,
        }
    }

    /// Rango en bytes de la primera coincidencia dentro de `hay`.
    fn find(&self, hay: &str) -> Option<(usize, usize)> {
        let mut pos = 0;
        let mut first = None;
        for segment in &self.segments {
            let (start, end) = segment.find(hay, pos)?;
            first.get_or_insert(start);
            pos = end;
        }
        Some((first.unwrap_or(0), pos))
    }
}

/// Minuscula de un caracter, siempre de un solo caracter, para que el texto
/// convertido tenga la misma cantidad de caracteres que el original.
fn fold_char(c: char) -> char {
    if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

fn fold_into(src: &str, dst: &mut String) {
    if src.is_ascii() {
        let start = dst.len();
        dst.push_str(src);
        dst[start..].make_ascii_lowercase();
    } else {
        dst.extend(src.chars().map(fold_char));
    }
}

/// Convierte una posicion del texto en minusculas a la del texto original.
fn map_offset(orig: &str, folded: &str, offset: usize) -> usize {
    if orig.len() == folded.len() && orig.is_ascii() {
        return offset;
    }
    let chars = folded[..offset].chars().count();
    orig.char_indices().nth(chars).map_or(orig.len(), |(index, _)| index)
}

// ---------------------------------------------------------------------------
// Decodificacion y lectura
// ---------------------------------------------------------------------------

// Codificaciones soportadas:
// - UTF-8 (con o sin BOM): la normal en Linux y en editores modernos.
// - UTF-16 con BOM: la usan el Bloc de notas ("Unicode") y PowerShell 5 al
//   redirigir con `>`. Se convierte a UTF-8 al vuelo con `Utf16Reader`.
// - Windows-1252 / Latin-1: archivos antiguos de Windows con acentos. Se usa
//   como respaldo, linea por linea, cuando una linea no es UTF-8 valido.

/// Bytes 0x80..0x9F de Windows-1252 (el resto coincide con Latin-1).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
    0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
    0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

fn cp1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => char::from_u32(u32::from(CP1252_HIGH[usize::from(b - 0x80)]))
                .unwrap_or('\u{FFFD}'),
            _ => char::from(b),
        })
        .collect()
}

/// UTF-8 si es valido; si no, Windows-1252 (archivos antiguos de Windows).
fn decode(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(cp1252(bytes)),
    }
}

/// Convierte un archivo UTF-16 (con BOM, tipico del Bloc de notas o de
/// PowerShell en Windows) a UTF-8 a medida que se lee.
struct Utf16Reader<R: Read> {
    inner: R,
    little_endian: bool,
    carry: Option<u8>,
    high: Option<u16>,
    out: Vec<u8>,
    pos: usize,
    eof: bool,
}

impl<R: Read> Utf16Reader<R> {
    fn new(inner: R, little_endian: bool) -> Self {
        Utf16Reader {
            inner,
            little_endian,
            carry: None,
            high: None,
            out: Vec::new(),
            pos: 0,
            eof: false,
        }
    }

    fn push_char(&mut self, c: char) {
        let mut tmp = [0u8; 4];
        self.out.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
    }

    fn push_unit(&mut self, unit: u16) {
        if let Some(high) = self.high.take() {
            if (0xDC00..0xE000).contains(&unit) {
                let code = 0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(unit) - 0xDC00);
                self.push_char(char::from_u32(code).unwrap_or('\u{FFFD}'));
                return;
            }
            self.push_char('\u{FFFD}');
        }
        match unit {
            0xD800..=0xDBFF => self.high = Some(unit),
            0xDC00..=0xDFFF => self.push_char('\u{FFFD}'),
            _ => self.push_char(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}')),
        }
    }
}

impl<R: Read> Read for Utf16Reader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.out.len() {
            if self.eof {
                return Ok(0);
            }
            self.out.clear();
            self.pos = 0;
            let mut raw = [0u8; 8192];
            let mut len = 0;
            if let Some(byte) = self.carry.take() {
                raw[0] = byte;
                len = 1;
            }
            let got = match self.inner.read(&mut raw[len..]) {
                Ok(got) => got,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    self.carry = if len == 1 { Some(raw[0]) } else { None };
                    continue;
                }
                Err(error) => return Err(error),
            };
            if got == 0 {
                self.eof = true;
                if self.high.take().is_some() || len == 1 {
                    self.push_char('\u{FFFD}');
                }
                continue;
            }
            len += got;
            let even = len - len % 2;
            if len % 2 == 1 {
                self.carry = Some(raw[len - 1]);
            }
            for &pair in raw[..even].as_chunks::<2>().0 {
                let unit = if self.little_endian {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                };
                self.push_unit(unit);
            }
        }
        let n = buf.len().min(self.out.len() - self.pos);
        buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Abre el archivo detectando la codificacion por su BOM. Devuelve tambien si
/// parece binario (contiene bytes nulos al inicio).
fn open_reader(path: &Path) -> io::Result<(Box<dyn BufRead>, bool)> {
    let mut reader = BufReader::with_capacity(64 * 1024, fs::File::open(path)?);
    let head = loop {
        match reader.fill_buf() {
            Ok(head) => break head,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    };

    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        let little_endian = head[0] == 0xFF;
        reader.consume(2);
        return Ok((
            Box::new(BufReader::new(Utf16Reader::new(reader, little_endian))),
            false,
        ));
    }
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
        reader.consume(3);
        return Ok((Box::new(reader), false));
    }
    let binary = head[..head.len().min(BINARY_PROBE)].contains(&0);
    Ok((Box::new(reader), binary))
}

/// Agrega a `buf` bytes de `reader` hasta encontrar un salto de linea, llegar
/// al final del archivo o completar `limit` bytes en `buf`.
/// Devuelve `true` si la linea termino (salto de linea o fin de archivo) y
/// `false` si se corto por el limite y quedan bytes de la misma linea.
// Se usa en vez de `BufRead::read_until` porque este no tiene limite y una
// linea gigante (o un archivo sin saltos de linea) llenaria la memoria.
fn read_line_limited(reader: &mut dyn BufRead, buf: &mut Vec<u8>, limit: usize) -> io::Result<bool> {
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(true);
        }
        let take = available.len().min(limit.saturating_sub(buf.len()));
        if let Some(pos) = available[..take].iter().position(|&byte| byte == b'\n') {
            buf.extend_from_slice(&available[..=pos]);
            reader.consume(pos + 1);
            return Ok(true);
        }
        buf.extend_from_slice(&available[..take]);
        reader.consume(take);
        if buf.len() >= limit {
            return Ok(false);
        }
    }
}

/// Descarta lo que queda de la linea actual sin guardarlo.
fn skip_rest_of_line(reader: &mut dyn BufRead) -> io::Result<()> {
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(());
        }
        if let Some(pos) = available.iter().position(|&byte| byte == b'\n') {
            reader.consume(pos + 1);
            return Ok(());
        }
        let len = available.len();
        reader.consume(len);
    }
}

// ---------------------------------------------------------------------------
// Salida
// ---------------------------------------------------------------------------

/// Neutraliza caracteres de control (ESC, BEL, CR, C1...) y de direccion de
/// texto (bidi) para que el contenido de los archivos o sus nombres no pueda
/// inyectar secuencias de escape en la terminal ni falsear lo que se ve.
fn sanitize(text: &str) -> Cow<'_, str> {
    fn dangerous(c: char) -> bool {
        (c.is_control() && c != '\t')
            || matches!(
                c,
                '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
    }
    if !text.chars().any(dangerous) {
        return Cow::Borrowed(text);
    }
    let mut clean = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if dangerous(c) {
            clean.push_str(&format!("\\x{{{:02X}}}", c as u32));
        } else {
            clean.push(c);
        }
    }
    Cow::Owned(clean)
}

fn show(path: &Path) -> String {
    sanitize(&path.to_string_lossy()).into_owned()
}

// Formatos de salida:
// - Terminal: un encabezado por archivo (nombre y carpeta) y debajo cada
//   linea con su numero, con la coincidencia resaltada en rojo.
// - Redirigida (`> archivo`, `| otro_programa`): una linea por coincidencia
//   con el formato `ruta_completa:numero_de_linea:contenido`, sin colores,
//   facil de procesar con otras herramientas.

/// Encabezado con el nombre y la carpeta del archivo (solo en la terminal).
fn write_heading(out: &mut Vec<u8>, path: &Path) -> io::Result<()> {
    let name = path
        .file_name()
        .map_or_else(|| show(path), |name| sanitize(&name.to_string_lossy()).into_owned());
    let folder = path.parent().map_or_else(String::new, show);
    writeln!(out, "\x1b[1;35mArchivo:\x1b[0m \x1b[1m{name}\x1b[0m")?;
    writeln!(out, "\x1b[35mCarpeta:\x1b[0m {folder}")
}

/// Donde esta una coincidencia: numero de linea en los archivos de texto o
/// una descripcion en los documentos ("pagina 3", "hoja 'Ventas', fila 7").
enum Loc<'a> {
    Line(usize),
    Doc(&'a str),
}

#[allow(clippy::too_many_arguments)]
fn write_match(
    out: &mut Vec<u8>,
    cfg: &Config,
    path: &Path,
    loc: Loc,
    prefix: &str,
    text: &str,
    start: usize,
    end: usize,
    suffix: &str,
) -> io::Result<()> {
    if cfg.color {
        if out.is_empty() {
            write_heading(out, path)?;
        }
        let prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("\x1b[2m{prefix}\x1b[0m")
        };
        let loc = match loc {
            Loc::Line(number) => format!("linea {number}"),
            Loc::Doc(place) => sanitize(place).into_owned(),
        };
        writeln!(
            out,
            "  \x1b[32m{loc}\x1b[0m: {prefix}{}\x1b[1;31m{}\x1b[0m{}{suffix}",
            sanitize(&text[..start]),
            sanitize(&text[start..end]),
            sanitize(&text[end..])
        )
    } else {
        let loc = match loc {
            Loc::Line(number) => number.to_string(),
            Loc::Doc(place) => sanitize(place).into_owned(),
        };
        writeln!(out, "{}:{loc}:{prefix}{}{suffix}", show(path), sanitize(text))
    }
}

/// Informa un archivo sin mostrar lineas: con -l o cuando es binario.
fn write_file_only(out: &mut Vec<u8>, cfg: &Config, path: &Path) -> io::Result<()> {
    if cfg.files_only {
        writeln!(out, "{}", show(path))
    } else if cfg.color {
        write_heading(out, path)?;
        writeln!(out, "  \x1b[2m[archivo binario: contiene el texto]\x1b[0m")
    } else {
        writeln!(out, "{}: [archivo binario: contiene el texto]", show(path))
    }
}

// ---------------------------------------------------------------------------
// Busqueda en un archivo
// ---------------------------------------------------------------------------

/// Coincidencia encontrada en una linea larga: el fragmento a mostrar, la
/// posicion de la coincidencia dentro de el y si se recorto texto antes o
/// despues.
struct Hit {
    text: String,
    start: usize,
    end: usize,
    cut_before: bool,
    cut_after: bool,
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Limites del fragmento a mostrar alrededor de `start..end`.
fn excerpt_bounds(text: &str, start: usize, end: usize) -> (usize, usize, usize) {
    let from = text[..start]
        .char_indices()
        .rev()
        .nth(CONTEXT_CHARS - 1)
        .map_or(0, |(index, _)| index);
    let end = floor_boundary(text, end.min(start + CONTEXT_CHARS * 4)).max(start);
    let to = text[end..]
        .char_indices()
        .nth(CONTEXT_CHARS)
        .map_or(text.len(), |(index, _)| end + index);
    (from, to, end)
}

/// Busca en una linea que supera `MAX_LINE_BYTES` procesandola por bloques.
/// `buf` trae el primer bloque. Los segmentos del patron se buscan en orden a
/// medida que llegan los bloques, asi que `*` puede abarcar varios bloques.
fn search_long_line(reader: &mut dyn BufRead, buf: &mut Vec<u8>, pat: &Pattern) -> io::Result<Option<Hit>> {
    let mut orig = String::new();
    let mut folded = String::new();
    let mut carry: Vec<u8> = Vec::new();
    let mut seg_index = 0;
    let mut pos = 0;
    let mut first: Option<usize> = None;
    let mut dropped = false;
    let mut complete = false;
    let keep = pat.max_chars + CONTEXT_CHARS;

    loop {
        let mut bytes = std::mem::take(&mut carry);
        bytes.extend_from_slice(buf);
        buf.clear();
        if complete && bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }

        // `orig` guarda el texto original del bloque (para mostrarlo) y
        // `folded` la misma ventana en minusculas (para buscar con -i). Ambos
        // tienen la misma cantidad de caracteres, asi que las posiciones se
        // pueden convertir de uno a otro contando caracteres.
        let added = orig.len();
        match std::str::from_utf8(&bytes) {
            Ok(text) => orig.push_str(text),
            Err(error) if error.error_len().is_none() && !complete => {
                // Caracter multibyte cortado al final del bloque: se completa
                // con el bloque siguiente.
                let valid = error.valid_up_to();
                orig.push_str(std::str::from_utf8(&bytes[..valid]).unwrap_or_default());
                carry = bytes[valid..].to_vec();
            }
            Err(_) => orig.push_str(&cp1252(&bytes)),
        }
        if pat.ignore_case {
            fold_into(&orig[added..], &mut folded);
        }

        // Se continua buscando los segmentos que faltan desde donde quedo el
        // bloque anterior: `seg_index` y `pos` sobreviven entre bloques.
        let hay = if pat.ignore_case { folded.as_str() } else { orig.as_str() };
        while seg_index < pat.segments.len() {
            match pat.segments[seg_index].find(hay, pos) {
                Some((start, end)) => {
                    first.get_or_insert(start);
                    seg_index += 1;
                    pos = end;
                }
                None => break,
            }
        }

        if seg_index == pat.segments.len() {
            if !complete {
                // Coincidencia al final del bloque: se lee un poco mas de la
                // linea para mostrar contexto despues de ella.
                let mut extra = std::mem::take(&mut carry);
                complete = read_line_limited(reader, &mut extra, CONTEXT_CHARS * 4)?;
                if complete && extra.last() == Some(&b'\n') {
                    extra.pop();
                    if extra.last() == Some(&b'\r') {
                        extra.pop();
                    }
                }
                let added = orig.len();
                orig.push_str(&String::from_utf8_lossy(&extra));
                if pat.ignore_case {
                    fold_into(&orig[added..], &mut folded);
                }
            }
            let start = first.unwrap_or(pos);
            let (start, end) = if pat.ignore_case {
                (map_offset(&orig, &folded, start), map_offset(&orig, &folded, pos))
            } else {
                (start, pos)
            };
            let (from, to, end) = excerpt_bounds(&orig, start, end);
            let hit = Hit {
                text: orig[from..to].to_string(),
                start: start - from,
                end: end - from,
                cut_before: dropped || from > 0,
                cut_after: to < orig.len() || !complete,
            };
            if !complete {
                skip_rest_of_line(reader)?;
            }
            return Ok(Some(hit));
        }
        if complete {
            return Ok(None);
        }

        // Se descarta lo ya revisado, conservando lo justo para no perder una
        // coincidencia que cruce el corte y algo de contexto.
        let cut = hay.char_indices().rev().nth(keep - 1).map_or(0, |(index, _)| index);
        if cut > 0 {
            if pat.ignore_case {
                let ocut = map_offset(&orig, &folded, cut);
                orig.drain(..ocut);
                folded.drain(..cut);
            } else {
                orig.drain(..cut);
            }
            pos = pos.saturating_sub(cut);
            first = first.map(|f| f.saturating_sub(cut));
            dropped = true;
        }
        complete = read_line_limited(reader, buf, MAX_LINE_BYTES)?;
    }
}

/// Busca en un archivo y escribe los resultados en `out`. Devuelve la
/// cantidad de lineas con coincidencias.
fn search_file(path: &Path, cfg: &Config, pat: &Pattern, out: &mut Vec<u8>) -> io::Result<usize> {
    // Los documentos se reconocen por la extension y se leen aparte.
    if let Some(kind) = docs::kind_of(path) {
        return search_document(path, kind, cfg, pat, out);
    }
    let (mut reader, binary) = open_reader(path)?;
    let mut buf: Vec<u8> = Vec::with_capacity(256);
    let mut folded = String::new();
    let mut line_number = 0usize;
    let mut matches = 0usize;

    loop {
        buf.clear();
        let complete = read_line_limited(reader.as_mut(), &mut buf, MAX_LINE_BYTES)?;
        if buf.is_empty() {
            return Ok(matches);
        }
        line_number += 1;

        if !complete {
            if let Some(hit) = search_long_line(reader.as_mut(), &mut buf, pat)? {
                matches += 1;
                if binary || cfg.files_only {
                    write_file_only(out, cfg, path)?;
                    return Ok(matches);
                }
                let prefix = format!(
                    "[linea de mas de {} MiB, recortada] {}",
                    MAX_LINE_BYTES / (1024 * 1024),
                    if hit.cut_before { "..." } else { "" }
                );
                let suffix = if hit.cut_after { "..." } else { "" };
                write_match(out, cfg, path, Loc::Line(line_number), &prefix, &hit.text, hit.start, hit.end, suffix)?;
            }
            continue;
        }

        // Linea normal: se quita el salto de linea (`\n` o `\r\n` de
        // Windows), se decodifica y se busca el patron.
        let content = buf.strip_suffix(b"\n").unwrap_or(&buf);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        let line = decode(content);
        let hay: &str = if pat.ignore_case {
            folded.clear();
            fold_into(&line, &mut folded);
            &folded
        } else {
            &line
        };

        let Some((start, end)) = pat.find(hay) else {
            continue;
        };
        matches += 1;
        if binary || cfg.files_only {
            write_file_only(out, cfg, path)?;
            return Ok(matches);
        }
        // Las posiciones se calcularon sobre el texto en minusculas; para
        // resaltar hay que pasarlas al texto original.
        let (start, end) = if pat.ignore_case && cfg.color {
            (map_offset(&line, &folded, start), map_offset(&line, &folded, end))
        } else {
            (start, end)
        };
        write_match(out, cfg, path, Loc::Line(line_number), "", &line, start, end, "")?;
    }
}

/// Unidades de texto de documento mas largas que esto se muestran recortadas
/// alrededor de la coincidencia (por ejemplo, un parrafo enorme).
const LONG_UNIT_BYTES: usize = 4096;

/// Busca en un documento: primero se extrae su texto en unidades (parrafos,
/// filas, lineas de pagina) y luego se busca en cada una igual que en las
/// lineas de un archivo de texto.
fn search_document(
    path: &Path,
    kind: docs::Kind,
    cfg: &Config,
    pat: &Pattern,
    out: &mut Vec<u8>,
) -> io::Result<usize> {
    let units = docs::extract(path, kind)?;
    let mut folded = String::new();
    let mut matches = 0usize;

    for unit in &units {
        let hay: &str = if pat.ignore_case {
            folded.clear();
            fold_into(&unit.text, &mut folded);
            &folded
        } else {
            &unit.text
        };
        let Some((start, end)) = pat.find(hay) else {
            continue;
        };
        matches += 1;
        if cfg.files_only {
            write_file_only(out, cfg, path)?;
            return Ok(matches);
        }
        let (start, end) = if pat.ignore_case {
            (map_offset(&unit.text, &folded, start), map_offset(&unit.text, &folded, end))
        } else {
            (start, end)
        };
        let loc = Loc::Doc(&unit.location);
        if unit.text.len() > LONG_UNIT_BYTES {
            let (from, to, end) = excerpt_bounds(&unit.text, start, end);
            let prefix = if from > 0 { "..." } else { "" };
            let suffix = if to < unit.text.len() { "..." } else { "" };
            write_match(out, cfg, path, loc, prefix, &unit.text[from..to], start - from, end - from, suffix)?;
        } else {
            write_match(out, cfg, path, loc, "", &unit.text, start, end, "")?;
        }
    }
    Ok(matches)
}

// ---------------------------------------------------------------------------
// Recorrido de carpetas y busqueda en paralelo
// ---------------------------------------------------------------------------

/// Elemento pendiente en la pila del recorrido.
enum Item {
    Dir(PathBuf),
    File(PathBuf),
}

/// Recorre las carpetas (sin recursion, para no agotar la pila) en orden
/// alfabetico y envia cada archivo a los hilos de busqueda.
fn walk(root: &Path, cfg: &Config, jobs: SyncSender<(usize, PathBuf)>, stop: &AtomicBool) {
    let mut stack = vec![Item::Dir(root.to_path_buf())];
    let mut index = 0;

    while let Some(item) = stack.pop() {
        match item {
            Item::File(path) => {
                if stop.load(Ordering::Relaxed) || jobs.send((index, path)).is_err() {
                    return;
                }
                index += 1;
            }
            Item::Dir(dir) => {
                let entries = match fs::read_dir(&dir) {
                    Ok(entries) => entries,
                    Err(error) => {
                        warn!("aviso: no se pudo acceder a {}: {error}", show(&dir));
                        continue;
                    }
                };
                let mut children = Vec::new();
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) => {
                            warn!("aviso: no se pudo leer una entrada en {}: {error}", show(&dir));
                            continue;
                        }
                    };
                    let path = entry.path();
                    let file_type = match entry.file_type() {
                        Ok(file_type) => file_type,
                        Err(error) => {
                            warn!("aviso: no se pudo leer {}: {error}", show(&path));
                            continue;
                        }
                    };
                    // No se siguen enlaces simbolicos ni uniones de Windows.
                    if file_type.is_symlink() {
                        continue;
                    }
                    if file_type.is_dir() {
                        children.push((entry.file_name(), Item::Dir(path)));
                    } else if file_type.is_file() && cfg.matches_extension(&path) {
                        children.push((entry.file_name(), Item::File(path)));
                    }
                }
                // Se ordena alfabeticamente y se apila al reves, para que al
                // sacar de la pila salgan en orden (igual que un recorrido
                // recursivo, pero sin riesgo de desbordar la pila).
                children.sort_by(|a, b| a.0.cmp(&b.0));
                stack.extend(children.into_iter().rev().map(|(_, item)| item));
            }
        }
    }
}

/// Resultado de buscar en un archivo, ya formateado, listo para imprimir.
struct FileResult {
    output: Vec<u8>,
    matches: usize,
    error: Option<String>,
}

/// Totales para el resumen final que se muestra en la terminal.
#[derive(Default)]
struct Summary {
    files: usize,
    lines: usize,
}

/// Imprime el resultado de un archivo y actualiza los totales.
fn emit<W: Write>(out: &mut W, cfg: &Config, result: &FileResult, summary: &mut Summary) -> io::Result<()> {
    if let Some(error) = &result.error {
        warn!("{error}");
    }
    if result.matches > 0 {
        summary.files += 1;
        summary.lines += result.matches;
    }
    if !result.output.is_empty() {
        out.write_all(&result.output)?;
        if cfg.color {
            if !cfg.files_only {
                out.write_all(b"\n")?;
            }
            out.flush()?;
        }
    }
    Ok(())
}

/// Ejecuta la busqueda completa. Devuelve los totales o un mensaje de error
/// fatal (ruta inexistente, salida que no se puede escribir...).
fn run<W: Write>(cfg: &Config, pat: &Pattern, out: &mut W) -> Result<Summary, String> {
    let mut summary = Summary::default();
    let metadata = fs::metadata(&cfg.root).map_err(|error| {
        let shown = show(&cfg.root);
        if shown.contains(['*', '?']) {
            format!(
                "la ruta no existe: {shown}. Los comodines van en el texto a buscar, \
                 no en la ruta; para elegir tipos de archivo usa -e (ej: -e txt,log)"
            )
        } else {
            format!("no se pudo acceder a {shown}: {error}")
        }
    })?;

    let write_error = |error: io::Error| format!("no se pudo escribir la salida: {error}");

    if metadata.is_file() {
        let mut output = Vec::new();
        let matches = search_file(&cfg.root, cfg, pat, &mut output)
            .map_err(|error| format!("no se pudo leer {}: {error}", show(&cfg.root)))?;
        let result = FileResult { output, matches, error: None };
        return match emit(out, cfg, &result, &mut summary) {
            Err(error) if !is_broken_pipe(&error) => Err(write_error(error)),
            _ => Ok(summary),
        };
    }
    if !metadata.is_dir() {
        return Err(format!("no es un archivo ni una carpeta: {}", show(&cfg.root)));
    }
    fs::read_dir(&cfg.root).map_err(|error| format!("no se pudo acceder a {}: {error}", show(&cfg.root)))?;

    // Busqueda en paralelo:
    //
    //   walk ──(indice, ruta)──▶ cola de trabajos ──▶ N hilos search_file
    //                                                        │
    //   hilo principal ◀──────(indice, resultado)────────────┘
    //
    // - La cola de trabajos tiene capacidad limitada (`sync_channel`) para
    //   que el recorrido no acumule millones de rutas en memoria.
    // - Cada archivo lleva su numero de orden; el hilo principal guarda en
    //   `pending` los que llegan antes de tiempo y los imprime en orden.
    // - `stop` se activa si no se puede seguir escribiendo (por ejemplo,
    //   `buscatxt ... | head`): los hilos vacian la cola sin trabajar y el
    //   recorrido termina, evitando que el programa se quede bloqueado.
    // - `thread::scope` permite que los hilos usen `cfg` y `pat` prestados
    //   y garantiza que todos terminen antes de salir de la funcion.
    let (job_tx, job_rx) = mpsc::sync_channel::<(usize, PathBuf)>(cfg.threads * 64);
    let job_rx = Mutex::new(job_rx);
    let (result_tx, result_rx) = mpsc::channel::<(usize, FileResult)>();
    let stop = AtomicBool::new(false);
    let mut failure: Option<io::Error> = None;

    thread::scope(|scope| {
        let job_rx = &job_rx;
        let stop = &stop;
        scope.spawn(move || walk(&cfg.root, cfg, job_tx, stop));

        for _ in 0..cfg.threads {
            let result_tx = result_tx.clone();
            scope.spawn(move || loop {
                let job = match job_rx.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => break,
                };
                let Ok((index, path)) = job else { break };
                if stop.load(Ordering::Relaxed) {
                    continue; // vaciar la cola para que el recorrido termine
                }
                let mut output = Vec::new();
                let result = match search_file(&path, cfg, pat, &mut output) {
                    Ok(matches) => FileResult { output, matches, error: None },
                    Err(error) => FileResult {
                        output: Vec::new(),
                        matches: 0,
                        error: Some(format!("aviso: no se pudo leer {}: {error}", show(&path))),
                    },
                };
                if result_tx.send((index, result)).is_err() {
                    stop.store(true, Ordering::Relaxed);
                }
            });
        }
        drop(result_tx);

        // Los resultados llegan en cualquier orden; se muestran en el orden
        // del recorrido para que la salida sea siempre la misma.
        let mut pending: HashMap<usize, FileResult> = HashMap::new();
        let mut next = 0;
        'receive: for (index, result) in result_rx {
            pending.insert(index, result);
            while let Some(result) = pending.remove(&next) {
                next += 1;
                if let Err(error) = emit(out, cfg, &result, &mut summary) {
                    failure = Some(error);
                    stop.store(true, Ordering::Relaxed);
                    break 'receive;
                }
            }
        }
    });

    match failure {
        Some(error) if !is_broken_pipe(&error) => Err(write_error(error)),
        _ => Ok(summary),
    }
}

/// Punto de entrada: lee los argumentos, ejecuta la busqueda, muestra el
/// resumen (solo en la terminal) y sale con el codigo correspondiente.
fn main() {
    // Un fallo inesperado al leer un documento se informa como aviso de ese
    // archivo (ver `docs::extract`) en vez de imprimir un panico.
    docs::install_panic_hook();
    let cfg = match parse_args() {
        Ok(cfg) => cfg,
        Err(message) => {
            warn!("error: {message}");
            warn!("Usa 'buscatxt --help' para ver la ayuda.");
            process::exit(2);
        }
    };
    let pattern = Pattern::new(&cfg.pattern, cfg.wildcards, cfg.ignore_case);

    // Salida con buffer: escribir linea por linea directo a la consola es
    // lento, sobre todo en Windows.
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let result = run(&cfg, &pattern, &mut out);
    let flushed = out.flush();

    match result {
        Ok(summary) => {
            if cfg.color && flushed.is_ok() {
                if summary.files == 0 {
                    warn!("Sin coincidencias.");
                } else if !cfg.files_only {
                    warn!(
                        "{} {} en {} {}.",
                        summary.lines,
                        if summary.lines == 1 { "linea encontrada" } else { "lineas encontradas" },
                        summary.files,
                        if summary.files == 1 { "archivo" } else { "archivos" }
                    );
                }
            }
            process::exit(if summary.files > 0 { 0 } else { 1 });
        }
        Err(message) => {
            warn!("error: {message}");
            process::exit(2);
        }
    }
}
