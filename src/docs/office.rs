//! Texto de documentos Office Open XML (DOCX, XLSX, PPTX) y OpenDocument de
//! LibreOffice (ODT, ODS, ODP).
//!
//! Ambos formatos son un ZIP con archivos XML dentro:
//!
//! | Formato | Partes que se leen |
//! |---|---|
//! | DOCX | `word/document.xml`, encabezados, pies, notas y comentarios |
//! | XLSX | `xl/workbook.xml`, `xl/sharedStrings.xml`, cada hoja |
//! | PPTX | `ppt/presentation.xml`, cada diapositiva y sus notas |
//! | ODT/ODS/ODP | `content.xml` (y `styles.xml` para encabezados de ODT) |
//!
//! Las imagenes y demas archivos del ZIP no se leen.

use std::collections::HashMap;
use std::io;

use super::xml::{self, local, Event};
use super::zip::Archive;
use super::{invalid, Kind, Unit, MAX_DOC_BYTES, MAX_PART_BYTES};

/// Documento abierto con un presupuesto total de bytes descomprimidos.
struct Doc {
    zip: Archive,
    budget: usize,
}

impl Doc {
    /// Lee una parte XML del documento como texto. `None` si no existe.
    fn part(&mut self, name: &str) -> io::Result<Option<String>> {
        let limit = MAX_PART_BYTES.min(self.budget);
        let Some(data) = self.zip.read(name, limit)? else {
            return Ok(None);
        };
        self.budget = self.budget.saturating_sub(data.len());
        Ok(Some(match String::from_utf8(data) {
            Ok(text) => text,
            Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
        }))
    }

    /// Nombres de las entradas del ZIP que cumplen una condicion, ordenados.
    fn names(&self, filter: impl Fn(&str) -> bool) -> Vec<String> {
        let mut names: Vec<String> = self
            .zip
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .filter(|name| filter(name))
            .collect();
        names.sort_by_key(|name| natural_key(name));
        names
    }
}

/// Clave de orden "natural": `slide2` antes que `slide10`.
fn natural_key(name: &str) -> (String, u64) {
    let digits: String = name
        .chars()
        .rev()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let prefix = name.trim_end_matches(|c: char| !c.is_ascii_digit());
    let prefix = prefix
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .to_string();
    (prefix, digits.parse().unwrap_or(0))
}

pub fn extract(path: &std::path::Path, kind: Kind) -> io::Result<Vec<Unit>> {
    let mut doc = Doc {
        zip: Archive::open(path)?,
        budget: MAX_DOC_BYTES,
    };
    let mut units = Vec::new();
    match kind {
        Kind::Docx => docx(&mut doc, &mut units)?,
        Kind::Xlsx => xlsx(&mut doc, &mut units)?,
        Kind::Pptx => pptx(&mut doc, &mut units)?,
        Kind::Odt | Kind::Ods | Kind::Odp => odf(&mut doc, kind, &mut units)?,
        Kind::Pdf | Kind::Doc | Kind::Xls | Kind::Ppt => {
            return Err(invalid("tipo de documento incorrecto"))
        }
    }
    Ok(units)
}

// ---------------------------------------------------------------------------
// Utilidades comunes de Office Open XML
// ---------------------------------------------------------------------------

/// Recorre los parrafos (`<w:p>` en Word, `<a:p>` en PowerPoint) y llama a
/// `emit` con el texto de cada uno que no este vacio.
///
/// - El texto visible esta en elementos `<w:t>` / `<a:t>`; el resto (estilos,
///   codigos de campo, texto borrado en el control de cambios) se ignora.
/// - `<mc:Fallback>` repite el contenido de `<mc:Choice>` para programas
///   antiguos; se salta para no duplicar el texto.
/// - Los parrafos pueden anidarse (cuadros de texto dentro de un parrafo),
///   por eso se usa una pila.
fn ooxml_paragraphs(text: &str, mut emit: impl FnMut(String)) {
    let mut stack: Vec<String> = Vec::new();
    let mut in_text = 0usize;
    let mut skip = 0usize;

    for event in xml::Reader::new(text) {
        match event {
            Event::Start { name, empty, .. } => {
                let name = local(name);
                if name == "Fallback" && !empty {
                    skip += 1;
                }
                if skip > 0 {
                    continue;
                }
                match name {
                    "p" if !empty => stack.push(String::new()),
                    "t" if !empty => in_text += 1,
                    "tab" | "ptab" => push_char(&mut stack, '\t'),
                    "br" | "cr" => push_char(&mut stack, ' '),
                    "noBreakHyphen" => push_char(&mut stack, '-'),
                    _ => {}
                }
            }
            Event::End { name } => {
                let name = local(name);
                if name == "Fallback" {
                    skip = skip.saturating_sub(1);
                    continue;
                }
                if skip > 0 {
                    continue;
                }
                match name {
                    "p" => {
                        if let Some(paragraph) = stack.pop() {
                            emit_trimmed(paragraph, &mut emit);
                        }
                    }
                    "t" => in_text = in_text.saturating_sub(1),
                    _ => {}
                }
            }
            Event::Text(content) => {
                if skip == 0 && in_text > 0 {
                    if let Some(top) = stack.last_mut() {
                        top.push_str(&content);
                    }
                }
            }
        }
    }
    // Parrafos sin cerrar (XML truncado): se entrega lo que haya.
    while let Some(paragraph) = stack.pop() {
        emit_trimmed(paragraph, &mut emit);
    }
}

fn push_char(stack: &mut [String], c: char) {
    if let Some(top) = stack.last_mut() {
        top.push(c);
    }
}

fn emit_trimmed(text: String, emit: &mut impl FnMut(String)) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        emit(if trimmed.len() == text.len() {
            text
        } else {
            trimmed.to_string()
        });
    }
}

/// Lee un archivo de relaciones (`*.rels`) y devuelve (id, tipo, destino),
/// con el destino convertido a una ruta dentro del ZIP.
fn relationships(text: &str, base_dir: &str) -> Vec<(String, String, String)> {
    let mut result = Vec::new();
    for event in xml::Reader::new(text) {
        if let Event::Start { name, attrs, .. } = event {
            if local(name) != "Relationship" {
                continue;
            }
            if xml::attr(attrs, "TargetMode").is_some_and(|mode| mode == "External") {
                continue;
            }
            let (Some(id), Some(target)) = (xml::attr(attrs, "Id"), xml::attr(attrs, "Target"))
            else {
                continue;
            };
            let kind = xml::attr(attrs, "Type").unwrap_or_default();
            result.push((
                id.into_owned(),
                kind.into_owned(),
                resolve(base_dir, &target),
            ));
        }
    }
    result
}

/// Une una ruta relativa a una carpeta del ZIP, resolviendo `..` y `.`.
fn resolve(base_dir: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None => format!("{base_dir}{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

/// Carpeta y archivo `.rels` asociados a una parte: `ppt/slides/slide1.xml`
/// -> (`ppt/slides/`, `ppt/slides/_rels/slide1.xml.rels`).
fn rels_path(part: &str) -> (String, String) {
    let (dir, file) = match part.rfind('/') {
        Some(index) => (&part[..=index], &part[index + 1..]),
        None => ("", part),
    };
    (dir.to_string(), format!("{dir}_rels/{file}.rels"))
}

// ---------------------------------------------------------------------------
// Word (DOCX)
// ---------------------------------------------------------------------------

fn docx(doc: &mut Doc, units: &mut Vec<Unit>) -> io::Result<()> {
    let main = doc
        .part("word/document.xml")?
        .ok_or_else(|| invalid("no es un documento DOCX valido (falta word/document.xml)"))?;
    let mut number = 0;
    ooxml_paragraphs(&main, |text| {
        number += 1;
        units.push(Unit {
            location: format!("parrafo {number}"),
            text,
        });
    });
    drop(main);

    // Otras partes con texto visible.
    let extra = doc.names(|name| {
        let Some(file) = name.strip_prefix("word/") else {
            return false;
        };
        !file.contains('/')
            && file.ends_with(".xml")
            && (file.starts_with("header")
                || file.starts_with("footer")
                || matches!(file, "footnotes.xml" | "endnotes.xml" | "comments.xml"))
    });
    for name in extra {
        let file = &name["word/".len()..];
        let location = if file.starts_with("header") {
            "encabezado"
        } else if file.starts_with("footer") {
            "pie de pagina"
        } else if file.starts_with("footnotes") {
            "nota al pie"
        } else if file.starts_with("endnotes") {
            "nota final"
        } else {
            "comentario"
        };
        if let Some(text) = doc.part(&name)? {
            ooxml_paragraphs(&text, |text| {
                units.push(Unit {
                    location: location.to_string(),
                    text,
                });
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PowerPoint (PPTX)
// ---------------------------------------------------------------------------

fn pptx(doc: &mut Doc, units: &mut Vec<Unit>) -> io::Result<()> {
    let presentation = doc.part("ppt/presentation.xml")?.ok_or_else(|| {
        invalid("no es una presentacion PPTX valida (falta ppt/presentation.xml)")
    })?;
    let rels = relationships(
        &doc.part("ppt/_rels/presentation.xml.rels")?
            .unwrap_or_default(),
        "ppt/",
    );

    // Orden de las diapositivas segun la presentacion (no segun el nombre
    // del archivo, que puede no coincidir si se reordenaron).
    let mut slides: Vec<String> = Vec::new();
    for event in xml::Reader::new(&presentation) {
        if let Event::Start { name, attrs, .. } = event {
            if local(name) == "sldId" {
                if let Some(id) = xml::prefixed_attr(attrs, "id") {
                    if let Some((_, _, target)) = rels.iter().find(|(rid, _, _)| *rid == id) {
                        slides.push(target.clone());
                    }
                }
            }
        }
    }
    if slides.is_empty() {
        slides = doc.names(|name| {
            name.starts_with("ppt/slides/slide")
                && name.ends_with(".xml")
                && !name.contains("/_rels/")
        });
    }

    for (index, slide) in slides.iter().enumerate() {
        let number = index + 1;
        let Some(text) = doc.part(slide)? else {
            continue;
        };
        ooxml_paragraphs(&text, |text| {
            units.push(Unit {
                location: format!("diapositiva {number}"),
                text,
            });
        });

        // Notas del orador de esta diapositiva.
        let (dir, rels_file) = rels_path(slide);
        let Some(slide_rels) = doc.part(&rels_file)? else {
            continue;
        };
        let notes = relationships(&slide_rels, &dir)
            .into_iter()
            .find(|(_, kind, _)| kind.ends_with("/notesSlide"));
        if let Some((_, _, notes_part)) = notes {
            if let Some(text) = doc.part(&notes_part)? {
                ooxml_paragraphs(&text, |text| {
                    // El marcador del numero de diapositiva no aporta nada.
                    if text != number.to_string() {
                        units.push(Unit {
                            location: format!("notas diapositiva {number}"),
                            text,
                        });
                    }
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Excel (XLSX)
// ---------------------------------------------------------------------------

/// Textos compartidos: Excel guarda cada texto una sola vez en
/// `sharedStrings.xml` y las celdas lo referencian por indice.
fn shared_strings(text: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    let mut phonetic = 0usize;
    for event in xml::Reader::new(text) {
        match event {
            Event::Start { name, empty, .. } => match local(name) {
                "si" if !empty => current = Some(String::new()),
                "si" => strings.push(String::new()),
                // Lectura fonetica (japones): no es texto visible.
                "rPh" if !empty => phonetic += 1,
                "t" if !empty => in_text = true,
                _ => {}
            },
            Event::End { name } => match local(name) {
                "si" => strings.push(current.take().unwrap_or_default()),
                "rPh" => phonetic = phonetic.saturating_sub(1),
                "t" => in_text = false,
                _ => {}
            },
            Event::Text(content) => {
                if in_text && phonetic == 0 {
                    if let Some(current) = current.as_mut() {
                        current.push_str(&content);
                    }
                }
            }
        }
    }
    strings
}

/// Que muestra un formato de numero de Excel: un numero normal, una fecha,
/// una hora o fecha y hora.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum NumberKind {
    Plain,
    Date,
    Time,
    DateTime,
}

/// Clasifica un codigo de formato (`yyyy-mm-dd`, `h:mm`, `#,##0.00`...).
/// Se ignora el texto entre comillas y entre corchetes (`[Red]`, `"kg"`).
pub(super) fn classify_format(code: &str) -> NumberKind {
    let mut date = false;
    let mut time = false;
    let mut in_quotes = false;
    let mut in_brackets = false;
    let mut escaped = false;
    for c in code.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '"' => in_quotes = !in_quotes,
            '[' if !in_quotes => in_brackets = true,
            ']' if !in_quotes => in_brackets = false,
            _ if in_quotes || in_brackets => {}
            'y' | 'Y' | 'd' | 'D' => date = true,
            'h' | 'H' | 's' | 'S' => time = true,
            _ => {}
        }
    }
    match (date, time) {
        (true, true) => NumberKind::DateTime,
        (true, false) => NumberKind::Date,
        (false, true) => NumberKind::Time,
        (false, false) => NumberKind::Plain,
    }
}

/// Lee `xl/styles.xml` y devuelve, para cada estilo de celda (atributo `s`
/// de la celda), que tipo de numero muestra.
fn cell_formats(text: &str) -> Vec<NumberKind> {
    let mut custom: HashMap<u32, NumberKind> = HashMap::new();
    let mut formats = Vec::new();
    let mut in_cell_xfs = false;
    for event in xml::Reader::new(text) {
        match event {
            Event::Start { name, attrs, empty } => match local(name) {
                "numFmt" => {
                    let id = xml::attr(attrs, "numFmtId").and_then(|id| id.parse().ok());
                    let code = xml::attr(attrs, "formatCode").unwrap_or_default();
                    if let Some(id) = id {
                        custom.insert(id, classify_format(&code));
                    }
                }
                "cellXfs" if !empty => in_cell_xfs = true,
                "xf" if in_cell_xfs => {
                    let id: u32 = xml::attr(attrs, "numFmtId")
                        .and_then(|id| id.parse().ok())
                        .unwrap_or(0);
                    // Formatos predefinidos de Excel con fecha u hora.
                    let kind = match id {
                        14..=17 | 27..=31 | 34..=36 | 50..=58 => NumberKind::Date,
                        18..=21 | 32 | 33 | 45..=47 => NumberKind::Time,
                        22 => NumberKind::DateTime,
                        _ => custom.get(&id).copied().unwrap_or(NumberKind::Plain),
                    };
                    formats.push(kind);
                }
                _ => {}
            },
            Event::End { name } => {
                if local(name) == "cellXfs" {
                    in_cell_xfs = false;
                }
            }
            Event::Text(_) => {}
        }
    }
    formats
}

/// Convierte un numero de serie de Excel (dias desde 1899-12-30, o desde
/// 1904-01-01 en libros con `date1904`) a `aaaa-mm-dd`, `hh:mm:ss` o ambos.
pub(super) fn format_serial(value: &str, kind: NumberKind, date1904: bool) -> Option<String> {
    let serial: f64 = value.trim().parse().ok()?;
    if !serial.is_finite() || !(0.0..3_000_000.0).contains(&serial) {
        return None;
    }
    let mut days = serial.floor() as i64;
    let mut seconds = ((serial - serial.floor()) * 86_400.0).round() as i64;
    if seconds >= 86_400 {
        days += 1;
        seconds -= 86_400;
    }
    // Dias desde 1970-01-01 y conversion a fecha civil (algoritmo de
    // Howard Hinnant, "days_from_civil" inverso).
    let z = days + if date1904 { -24_107 } else { -25_569 } + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    let date = format!("{year:04}-{month:02}-{day:02}");
    let time = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    Some(match kind {
        NumberKind::Date if seconds == 0 => date,
        NumberKind::Date | NumberKind::DateTime => format!("{date} {time}"),
        NumberKind::Time => time,
        NumberKind::Plain => return None,
    })
}

fn xlsx(doc: &mut Doc, units: &mut Vec<Unit>) -> io::Result<()> {
    let workbook = doc
        .part("xl/workbook.xml")?
        .ok_or_else(|| invalid("no es una hoja de calculo XLSX valida (falta xl/workbook.xml)"))?;
    let rels = relationships(
        &doc.part("xl/_rels/workbook.xml.rels")?.unwrap_or_default(),
        "xl/",
    );
    let shared = shared_strings(&doc.part("xl/sharedStrings.xml")?.unwrap_or_default());
    // Las fechas se guardan como numeros; el estilo de la celda indica si
    // hay que mostrarlas como fecha.
    let formats = cell_formats(&doc.part("xl/styles.xml")?.unwrap_or_default());
    let date1904 = workbook.contains("date1904=\"1\"") || workbook.contains("date1904=\"true\"");

    let mut sheets: Vec<(String, String)> = Vec::new();
    for event in xml::Reader::new(&workbook) {
        if let Event::Start { name, attrs, .. } = event {
            if local(name) != "sheet" {
                continue;
            }
            let sheet_name = xml::attr(attrs, "name").unwrap_or_default().into_owned();
            if let Some(id) = xml::prefixed_attr(attrs, "id") {
                if let Some((_, _, target)) = rels.iter().find(|(rid, _, _)| *rid == id) {
                    sheets.push((sheet_name, target.clone()));
                }
            }
        }
    }

    for (sheet_name, target) in sheets {
        // Las hojas de graficos no tienen celdas; si no existe se ignora.
        let Some(text) = doc.part(&target)? else {
            continue;
        };
        let mut row_number = 0usize;
        let mut cells: Vec<String> = Vec::new();
        let mut cell_type = String::new();
        let mut cell_format = NumberKind::Plain;
        let mut value = String::new();
        let mut in_value = false;
        let mut phonetic = 0usize;

        for event in xml::Reader::new(&text) {
            match event {
                Event::Start { name, attrs, empty } => match local(name) {
                    "row" => {
                        row_number = xml::attr(attrs, "r")
                            .and_then(|r| r.parse().ok())
                            .unwrap_or(row_number + 1);
                        cells.clear();
                    }
                    "c" if !empty => {
                        cell_type = xml::attr(attrs, "t").unwrap_or_default().into_owned();
                        cell_format = xml::attr(attrs, "s")
                            .and_then(|s| s.parse::<usize>().ok())
                            .and_then(|s| formats.get(s).copied())
                            .unwrap_or(NumberKind::Plain);
                        value.clear();
                    }
                    "v" | "t" if !empty => in_value = true,
                    "rPh" if !empty => phonetic += 1,
                    _ => {}
                },
                Event::End { name } => match local(name) {
                    "v" | "t" => in_value = false,
                    "rPh" => phonetic = phonetic.saturating_sub(1),
                    "c" => {
                        let shown = match cell_type.as_str() {
                            "s" => value
                                .trim()
                                .parse::<usize>()
                                .ok()
                                .and_then(|index| shared.get(index).cloned())
                                .unwrap_or_default(),
                            "b" => String::from(if value.trim() == "1" {
                                "VERDADERO"
                            } else {
                                "FALSO"
                            }),
                            "" | "n" if cell_format != NumberKind::Plain => {
                                format_serial(&value, cell_format, date1904)
                                    .unwrap_or_else(|| std::mem::take(&mut value))
                            }
                            _ => std::mem::take(&mut value),
                        };
                        if !shown.trim().is_empty() {
                            cells.push(shown);
                        }
                        value.clear();
                    }
                    "row" if !cells.is_empty() => {
                        units.push(Unit {
                            location: format!("hoja '{sheet_name}', fila {row_number}"),
                            text: cells.join(" | "),
                        });
                        cells.clear();
                    }
                    _ => {}
                },
                Event::Text(content) => {
                    if in_value && phonetic == 0 {
                        value.push_str(&content);
                    }
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LibreOffice / OpenDocument (ODT, ODS, ODP)
// ---------------------------------------------------------------------------

fn odf(doc: &mut Doc, kind: Kind, units: &mut Vec<Unit>) -> io::Result<()> {
    // En los documentos protegidos, content.xml esta cifrado.
    if let Some(manifest) = doc.part("META-INF/manifest.xml")? {
        if manifest.contains("encryption-data") {
            return Err(invalid("documento protegido con contrasena"));
        }
    }
    let content = doc
        .part("content.xml")?
        .ok_or_else(|| invalid("no es un documento OpenDocument valido (falta content.xml)"))?;
    odf_content(&content, kind, units);

    // Encabezados y pies de pagina de los documentos de texto.
    if kind == Kind::Odt {
        if let Some(styles) = doc.part("styles.xml")? {
            odf_headers(&styles, units);
        }
    }
    Ok(())
}

/// Recorre `content.xml`. Los parrafos son `<text:p>` y `<text:h>`; segun el
/// tipo de documento se agrupan por parrafo (ODT), fila de hoja (ODS) o
/// diapositiva (ODP).
fn odf_content(text: &str, kind: Kind, units: &mut Vec<Unit>) {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut paragraph_number = 0usize;
    // Texto borrado guardado por el control de cambios: no es visible.
    let mut skip = 0usize;

    // Hojas de calculo (ODS).
    let mut sheet = String::new();
    let mut row = 0usize;
    let mut row_start = 0usize;
    let mut row_repeat = 1usize;
    let mut cells: Vec<String> = Vec::new();
    let mut cell: Option<String> = None;

    // Presentaciones (ODP).
    let mut page = 0usize;
    let mut notes = 0usize;

    // Notas al pie/finales y comentarios: se informan aparte, sin mezclarse
    // con el parrafo donde estan anclados ni contar como parrafo.
    let mut footnote = 0usize;
    let mut comment = 0usize;
    // Datos que no son texto visible del documento: el numero de llamada de
    // una nota, y el autor y la fecha de un comentario.
    let mut hidden = 0usize;

    for event in xml::Reader::new(text) {
        match event {
            Event::Start { name, attrs, empty } => {
                let name = local(name);
                if name == "tracked-changes" && !empty {
                    skip += 1;
                }
                if skip > 0 {
                    continue;
                }
                match name {
                    "note-body" if !empty => footnote += 1,
                    "annotation" if !empty => comment += 1,
                    "note-citation" | "creator" | "date" | "initials" if !empty => hidden += 1,
                    "p" | "h" if !empty => paragraphs.push(String::new()),
                    "s" => {
                        let count = xml::attr(attrs, "c")
                            .and_then(|c| c.parse::<usize>().ok())
                            .unwrap_or(1)
                            .min(1000);
                        if let Some(top) = paragraphs.last_mut() {
                            top.extend(std::iter::repeat_n(' ', count));
                        }
                    }
                    "tab" => push_char(&mut paragraphs, '\t'),
                    "line-break" => push_char(&mut paragraphs, ' '),
                    "table" if kind == Kind::Ods && !empty => {
                        sheet = xml::attr(attrs, "name").unwrap_or_default().into_owned();
                        row = 0;
                    }
                    "table-row" if kind == Kind::Ods => {
                        // Las filas vacias repetidas se guardan una sola vez
                        // con un contador; solo se usa para numerar.
                        row_repeat = xml::attr(attrs, "number-rows-repeated")
                            .and_then(|n| n.parse::<usize>().ok())
                            .unwrap_or(1)
                            .max(1);
                        row_start = row + 1;
                        cells.clear();
                        if empty {
                            row += row_repeat;
                        }
                    }
                    "table-cell" | "covered-table-cell" if kind == Kind::Ods && !empty => {
                        cell = Some(String::new());
                    }
                    "page" if kind == Kind::Odp && !empty => page += 1,
                    "notes" if kind == Kind::Odp && !empty => notes += 1,
                    _ => {}
                }
            }
            Event::End { name } => {
                let name = local(name);
                if name == "tracked-changes" {
                    skip = skip.saturating_sub(1);
                    continue;
                }
                if skip > 0 {
                    continue;
                }
                match name {
                    "p" | "h" => {
                        let Some(paragraph) = paragraphs.pop() else {
                            continue;
                        };
                        let paragraph = paragraph.trim();
                        if paragraph.is_empty() {
                            continue;
                        }
                        if comment > 0 || footnote > 0 {
                            units.push(Unit {
                                location: String::from(if comment > 0 {
                                    "comentario"
                                } else {
                                    "nota al pie"
                                }),
                                text: paragraph.to_string(),
                            });
                            continue;
                        }
                        if let Some(cell) = cell.as_mut() {
                            // Dentro de una celda: se junta con el resto de
                            // la celda y se informa por fila.
                            if !cell.is_empty() {
                                cell.push(' ');
                            }
                            cell.push_str(paragraph);
                            continue;
                        }
                        let location = match kind {
                            Kind::Odp if notes > 0 => format!("notas diapositiva {page}"),
                            Kind::Odp => format!("diapositiva {page}"),
                            Kind::Ods => format!("hoja '{sheet}'"),
                            _ => {
                                paragraph_number += 1;
                                format!("parrafo {paragraph_number}")
                            }
                        };
                        units.push(Unit {
                            location,
                            text: paragraph.to_string(),
                        });
                    }
                    "table-cell" | "covered-table-cell" if kind == Kind::Ods => {
                        if let Some(cell) = cell.take() {
                            if !cell.is_empty() {
                                cells.push(cell);
                            }
                        }
                    }
                    "table-row" if kind == Kind::Ods => {
                        if !cells.is_empty() {
                            units.push(Unit {
                                location: format!("hoja '{sheet}', fila {row_start}"),
                                text: cells.join(" | "),
                            });
                            cells.clear();
                        }
                        row += row_repeat;
                    }
                    "notes" if kind == Kind::Odp => notes = notes.saturating_sub(1),
                    "note-body" => footnote = footnote.saturating_sub(1),
                    "annotation" => comment = comment.saturating_sub(1),
                    "note-citation" | "creator" | "date" | "initials" => {
                        hidden = hidden.saturating_sub(1)
                    }
                    _ => {}
                }
            }
            Event::Text(content) => {
                if skip == 0 && hidden == 0 {
                    if let Some(top) = paragraphs.last_mut() {
                        top.push_str(&content);
                    }
                }
            }
        }
    }
}

/// Encabezados y pies de pagina de un ODT (estan en `styles.xml`).
fn odf_headers(text: &str, units: &mut Vec<Unit>) {
    let mut section: Vec<&'static str> = Vec::new();
    let mut paragraphs: Vec<String> = Vec::new();
    for event in xml::Reader::new(text) {
        match event {
            Event::Start { name, empty, .. } => match local(name) {
                "header" | "header-left" | "header-first" if !empty => section.push("encabezado"),
                "footer" | "footer-left" | "footer-first" if !empty => {
                    section.push("pie de pagina")
                }
                "p" | "h" if !empty && !section.is_empty() => paragraphs.push(String::new()),
                "tab" => push_char(&mut paragraphs, '\t'),
                "s" | "line-break" => push_char(&mut paragraphs, ' '),
                _ => {}
            },
            Event::End { name } => match local(name) {
                "header" | "header-left" | "header-first" | "footer" | "footer-left"
                | "footer-first" => {
                    section.pop();
                }
                "p" | "h" => {
                    if let (Some(paragraph), Some(location)) = (paragraphs.pop(), section.last()) {
                        let paragraph = paragraph.trim();
                        if !paragraph.is_empty() {
                            units.push(Unit {
                                location: location.to_string(),
                                text: paragraph.to_string(),
                            });
                        }
                    }
                }
                _ => {}
            },
            Event::Text(content) => {
                if let Some(top) = paragraphs.last_mut() {
                    top.push_str(&content);
                }
            }
        }
    }
}
