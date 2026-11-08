//! Extraccion de texto de documentos: Word/Excel/PowerPoint (DOCX, XLSX,
//! PPTX y los antiguos DOC, XLS, PPT), LibreOffice/OpenDocument (ODT, ODS,
//! ODP) y PDF.
//!
//! Cada documento se convierte en una lista de [`Unit`]: trozos de texto
//! (un parrafo, una fila de una hoja, una linea de una pagina...) con una
//! ubicacion legible ("pagina 3", "hoja 'Ventas', fila 12"). `main.rs`
//! busca el patron en cada unidad igual que en las lineas de un archivo de
//! texto.
//!
//! Todo esta escrito con la biblioteca estandar (sin dependencias externas)
//! y en Rust seguro. Como estos formatos son complejos y el archivo puede ser
//! malicioso, hay varias defensas:
//! - Limites de tamano en todo lo que se descomprime o se lee.
//! - El lector XML no procesa DTD ni entidades externas.
//! - Si aun asi el codigo de extraccion fallara con un panico, se captura y
//!   se informa como un aviso sobre ese archivo, sin detener la busqueda.

mod cfb;
mod inflate;
mod legacy;
mod office;
mod pdf;
mod xml;
mod zip;

use std::cell::Cell;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

/// Tamano maximo descomprimido de una parte de un documento (un XML dentro
/// de un DOCX, un flujo de un PDF...).
pub const MAX_PART_BYTES: usize = 128 * 1024 * 1024;

/// Tamano maximo descomprimido en total por documento.
pub const MAX_DOC_BYTES: usize = 512 * 1024 * 1024;

/// Trozo de texto de un documento con su ubicacion.
pub struct Unit {
    /// Donde esta el texto, por ejemplo "pagina 3" o "parrafo 12".
    pub location: String,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Docx,
    Xlsx,
    Pptx,
    Odt,
    Ods,
    Odp,
    Pdf,
    /// Formatos binarios de Office 97-2003.
    Doc,
    Xls,
    Ppt,
}

/// Extensiones de documento que se leen extrayendo su texto. Las usa la
/// opcion `-d/--documentos`.
pub const EXTENSIONS: &[&str] = &[
    "docx", "docm", "dotx", "dotm", "xlsx", "xlsm", "xltx", "xltm", "pptx", "pptm", "potx", "ppsx",
    "odt", "ott", "ods", "ots", "odp", "otp", "pdf", "doc", "dot", "xls", "xlt", "ppt", "pot",
    "pps",
];

/// Tipo de documento segun la extension del archivo.
pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "docx" | "docm" | "dotx" | "dotm" => Kind::Docx,
        "xlsx" | "xlsm" | "xltx" | "xltm" => Kind::Xlsx,
        "pptx" | "pptm" | "potx" | "ppsx" => Kind::Pptx,
        "odt" | "ott" => Kind::Odt,
        "ods" | "ots" => Kind::Ods,
        "odp" | "otp" => Kind::Odp,
        "pdf" => Kind::Pdf,
        "doc" | "dot" => Kind::Doc,
        "xls" | "xlt" => Kind::Xls,
        "ppt" | "pot" | "pps" => Kind::Ppt,
        _ => return None,
    })
}

/// Error de "datos invalidos" con un mensaje para el usuario.
pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

thread_local! {
    /// Indica que el hilo esta extrayendo un documento: si ocurre un panico,
    /// el gancho de panico no imprime nada (se informa como aviso).
    static EXTRACTING: Cell<bool> = const { Cell::new(false) };
}

/// Instala un gancho de panico que no imprime nada durante la extraccion de
/// documentos y mantiene el mensaje normal en cualquier otro caso.
pub fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if !EXTRACTING.with(Cell::get) {
            default_hook(info);
        }
    }));
}

/// Extrae el texto de un documento.
pub fn extract(path: &Path, kind: Kind) -> io::Result<Vec<Unit>> {
    EXTRACTING.with(|flag| flag.set(true));
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        // Un .docx/.xlsx/.pptx protegido con contrasena (o un .doc antiguo
        // renombrado) es un contenedor OLE, no un ZIP: se lee como formato
        // antiguo, que detecta el cifrado y lo informa.
        let ooxml = matches!(kind, Kind::Docx | Kind::Xlsx | Kind::Pptx);
        if ooxml && cfb::is_compound(path) {
            let legacy_kind = match kind {
                Kind::Docx => Kind::Doc,
                Kind::Xlsx => Kind::Xls,
                _ => Kind::Ppt,
            };
            return legacy::extract(path, legacy_kind).map_err(|error| {
                if error.kind() == io::ErrorKind::InvalidData {
                    invalid("documento protegido con contrasena o danado")
                } else {
                    error
                }
            });
        }
        match kind {
            Kind::Pdf => pdf::extract(path),
            Kind::Doc | Kind::Xls | Kind::Ppt => legacy::extract(path, kind),
            _ => office::extract(path, kind),
        }
    }));
    EXTRACTING.with(|flag| flag.set(false));
    result.unwrap_or_else(|_| {
        Err(invalid(
            "no se pudo extraer el texto (documento danado o con un formato no soportado)",
        ))
    })
}
