//! Texto de los formatos binarios de Office 97-2003: Word (`.doc`), Excel
//! (`.xls`) y PowerPoint (`.ppt`). Los tres van dentro de un contenedor CFB
//! (ver `cfb.rs`).
//!
//! - **Word**: el flujo `WordDocument` empieza con la cabecera FIB, que dice
//!   donde esta la "tabla de piezas" (en el flujo `0Table` o `1Table`). Cada
//!   pieza indica un tramo de texto y si esta en 8 bits (Windows-1252) o en
//!   UTF-16. El texto incluye, en este orden: cuerpo, notas al pie,
//!   encabezados y pies, comentarios, notas finales y cuadros de texto.
//! - **Excel**: el flujo `Workbook` es una secuencia de registros BIFF8. Los
//!   textos estan en una tabla compartida (registro SST) y las celdas los
//!   referencian; los numeros pueden estar comprimidos (formato RK).
//! - **PowerPoint**: el flujo `PowerPoint Document` es un arbol de
//!   registros. El texto esta en atomos `TextCharsAtom` (UTF-16) y
//!   `TextBytesAtom` (8 bits); el orden de las diapositivas sale de la lista
//!   `SlideListWithText` y del "directorio de persistencia", que dice en que
//!   posicion del flujo esta cada diapositiva.
//!
//! No se admiten las versiones anteriores a Office 97 (Word 6/95, Excel
//! 5/95) ni los documentos cifrados.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use super::cfb::Compound;
use super::office::{classify_format, format_serial, NumberKind};
use super::{invalid, Kind, Unit};

/// Cantidad maxima de caracteres de texto que se extraen de un documento.
const MAX_CHARS: usize = 64 * 1024 * 1024;

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

/// Windows-1252 de un byte (codificacion de 8 bits de Office).
fn cp1252_char(byte: u8) -> char {
    const HIGH: [u16; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];
    match byte {
        0x80..=0x9F => {
            char::from_u32(u32::from(HIGH[usize::from(byte - 0x80)])).unwrap_or('\u{FFFD}')
        }
        _ => char::from(byte),
    }
}

fn utf16le(bytes: &[u8]) -> String {
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u16::from_le_bytes([p[0], p[1]]));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or('\u{FFFD}'))
        .collect()
}

pub fn extract(path: &Path, kind: Kind) -> io::Result<Vec<Unit>> {
    let compound = Compound::open(path)?;
    if compound.has_stream("EncryptedPackage") || compound.has_stream("EncryptedSummary") {
        return Err(invalid("documento protegido con contrasena"));
    }
    // Un .docx/.xlsx protegido con contrasena tambien es un archivo CFB.
    let mut units = Vec::new();
    match kind {
        Kind::Doc => word(&compound, &mut units)?,
        Kind::Xls => excel(&compound, &mut units)?,
        Kind::Ppt => powerpoint(&compound, &mut units)?,
        _ => return Err(invalid("tipo de documento incorrecto")),
    }
    Ok(units)
}

// ---------------------------------------------------------------------------
// Word 97-2003 (.doc)
// ---------------------------------------------------------------------------

fn word(compound: &Compound, units: &mut Vec<Unit>) -> io::Result<()> {
    let bad = || invalid("documento de Word danado o con un formato no soportado");
    let document = compound
        .stream("WordDocument")?
        .ok_or_else(|| invalid("no es un documento de Word (.doc) valido"))?;

    // Cabecera FIB.
    if u16_at(&document, 0) != Some(0xA5EC) {
        return Err(bad());
    }
    let version = u16_at(&document, 2).ok_or_else(bad)?;
    if version < 0x00C1 {
        return Err(invalid(
            "formato de Word 6/95 no soportado (solo Word 97 o posterior)",
        ));
    }
    let flags = u16_at(&document, 0x0A).ok_or_else(bad)?;
    if flags & 0x0100 != 0 {
        return Err(invalid("documento protegido con contrasena"));
    }
    let table_name = if flags & 0x0200 != 0 {
        "1Table"
    } else {
        "0Table"
    };

    // Despues de la base (32 bytes) vienen tres bloques de largo variable:
    // valores de 16 bits, de 32 bits (cantidades de caracteres) y pares
    // posicion/largo (FcLcb).
    let csw = usize::from(u16_at(&document, 32).ok_or_else(bad)?);
    let lw_count_pos = 34 + csw * 2;
    let cslw = usize::from(u16_at(&document, lw_count_pos).ok_or_else(bad)?);
    let lw = lw_count_pos + 2;
    let fc_count_pos = lw + cslw * 4;
    let fc_lcb = fc_count_pos + 2;
    let ccp = |index: usize| u32_at(&document, lw + index * 4).unwrap_or(0) as usize;
    // ccpText, ccpFtn, ccpHdd, (reservado), ccpAtn, ccpEdn, ccpTxbx, ccpHdrTxbx
    let parts = [
        (ccp(3), "parrafo"),
        (ccp(4), "nota al pie"),
        (ccp(5), "encabezado o pie"),
        (ccp(6), ""),
        (ccp(7), "comentario"),
        (ccp(8), "nota final"),
        (ccp(9), "cuadro de texto"),
        (ccp(10), "cuadro de texto"),
    ];
    let fc_clx = u32_at(&document, fc_lcb + 33 * 8).ok_or_else(bad)? as usize;
    let lcb_clx = u32_at(&document, fc_lcb + 33 * 8 + 4).ok_or_else(bad)? as usize;

    let table = compound.stream(table_name)?.ok_or_else(bad)?;
    let clx = table
        .get(fc_clx..fc_clx.saturating_add(lcb_clx))
        .ok_or_else(bad)?;

    // Clx: registros Prc (0x01, formato) que se saltan, y luego la tabla de
    // piezas Pcdt (0x02).
    let mut pos = 0;
    while clx.get(pos) == Some(&0x01) {
        pos += 3 + usize::from(u16_at(clx, pos + 1).ok_or_else(bad)?);
    }
    if clx.get(pos) != Some(&0x02) {
        return Err(bad());
    }
    let plc_len = u32_at(clx, pos + 1).ok_or_else(bad)? as usize;
    let plc = clx.get(pos + 5..pos + 5 + plc_len).ok_or_else(bad)?;
    // PlcPcd: n+1 posiciones de caracter (4 bytes) y n descriptores (8 bytes).
    let pieces = plc_len.saturating_sub(4) / 12;

    let mut text = String::new();
    for i in 0..pieces {
        let cp_start = u32_at(plc, i * 4).ok_or_else(bad)? as usize;
        let cp_end = u32_at(plc, (i + 1) * 4).ok_or_else(bad)? as usize;
        let count = cp_end.saturating_sub(cp_start);
        let descriptor = (pieces + 1) * 4 + i * 8;
        let fc = u32_at(plc, descriptor + 2).ok_or_else(bad)?;
        if text.len() + count > MAX_CHARS {
            break;
        }
        if fc & 0x4000_0000 != 0 {
            // Texto de 8 bits: la posicion real es la mitad.
            let start = (fc & 0x3FFF_FFFF) as usize / 2;
            if let Some(bytes) = document.get(start..start + count) {
                text.extend(bytes.iter().map(|&b| cp1252_char(b)));
            }
        } else {
            let start = fc as usize;
            if let Some(bytes) = document.get(start..start + count * 2) {
                text.push_str(&utf16le(bytes));
            }
        }
    }

    // Se reparte el texto en sus partes y se separa en parrafos.
    let chars: Vec<char> = text.chars().collect();
    let mut offset = 0;
    let mut number = 0;
    // Word guarda una copia de los encabezados y pies por cada variante
    // (pagina impar, par, primera pagina) y seccion: se muestra cada texto
    // una sola vez.
    let mut seen_headers: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (length, label) in parts {
        let end = (offset + length).min(chars.len());
        if !label.is_empty() {
            for paragraph in word_paragraphs(&chars[offset.min(end)..end]) {
                if label == "encabezado o pie" && !seen_headers.insert(paragraph.clone()) {
                    continue;
                }
                let location = if label == "parrafo" {
                    number += 1;
                    format!("parrafo {number}")
                } else {
                    label.to_string()
                };
                units.push(Unit {
                    location,
                    text: paragraph,
                });
            }
        }
        offset = end;
    }
    Ok(())
}

/// Separa el texto de Word en parrafos, quitando los codigos de campo y los
/// caracteres especiales.
fn word_paragraphs(chars: &[char]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    // Campos: 0x13 instrucciones 0x14 resultado 0x15. Se muestra solo el
    // resultado (por ejemplo, el numero de pagina, no "PAGE").
    let mut field_stack: Vec<bool> = Vec::new();
    let flush = |current: &mut String, paragraphs: &mut Vec<String>| {
        let text = current.trim();
        if !text.is_empty() {
            paragraphs.push(text.to_string());
        }
        current.clear();
    };
    for &c in chars {
        match c {
            '\u{13}' => field_stack.push(true),
            '\u{14}' => {
                if let Some(top) = field_stack.last_mut() {
                    *top = false;
                }
            }
            '\u{15}' => {
                field_stack.pop();
            }
            _ if field_stack.last() == Some(&true) => {}
            // Fin de parrafo, fin de celda o de fila, salto de pagina o columna.
            '\r' | '\u{07}' | '\u{0C}' | '\u{0E}' => flush(&mut current, &mut paragraphs),
            '\u{0B}' => current.push(' '),
            '\u{1E}' => current.push('-'),
            '\t' => current.push('\t'),
            c if (c as u32) < 0x20 => {}
            c => current.push(c),
        }
    }
    flush(&mut current, &mut paragraphs);
    paragraphs
}

// ---------------------------------------------------------------------------
// Excel 97-2003 (.xls)
// ---------------------------------------------------------------------------

/// Registro BIFF: tipo y datos, con sus registros CONTINUE ya unidos como
/// trozos separados (hacen falta separados para leer bien los textos).
struct Record<'a> {
    kind: u16,
    chunks: Vec<&'a [u8]>,
}

impl Record<'_> {
    fn data(&self) -> &[u8] {
        self.chunks.first().copied().unwrap_or(&[])
    }
}

/// Lee los registros a partir de `start` hasta el registro EOF.
fn biff_records(stream: &[u8], start: usize) -> Vec<Record<'_>> {
    let mut records: Vec<Record> = Vec::new();
    let mut pos = start;
    while let (Some(kind), Some(len)) = (u16_at(stream, pos), u16_at(stream, pos + 2)) {
        let len = usize::from(len);
        let Some(data) = stream.get(pos + 4..pos + 4 + len) else {
            break;
        };
        pos += 4 + len;
        if kind == 0x003C {
            // CONTINUE: continua el registro anterior.
            if let Some(last) = records.last_mut() {
                last.chunks.push(data);
            }
            continue;
        }
        records.push(Record {
            kind,
            chunks: vec![data],
        });
        if kind == 0x000A {
            break; // EOF del bloque actual
        }
    }
    records
}

/// Lector de un registro dividido en trozos por registros CONTINUE.
struct Chunks<'a> {
    chunks: &'a [&'a [u8]],
    index: usize,
    pos: usize,
}

impl<'a> Chunks<'a> {
    fn new(chunks: &'a [&'a [u8]], pos: usize) -> Self {
        Chunks {
            chunks,
            index: 0,
            pos,
        }
    }

    fn byte(&mut self) -> Option<u8> {
        loop {
            let chunk = self.chunks.get(self.index)?;
            if let Some(&b) = chunk.get(self.pos) {
                self.pos += 1;
                return Some(b);
            }
            self.index += 1;
            self.pos = 0;
        }
    }

    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes([self.byte()?, self.byte()?]))
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes([
            self.byte()?,
            self.byte()?,
            self.byte()?,
            self.byte()?,
        ]))
    }

    fn skip(&mut self, mut count: usize) -> Option<()> {
        while count > 0 {
            let chunk = self.chunks.get(self.index)?;
            let available = chunk.len().saturating_sub(self.pos);
            if available == 0 {
                self.index += 1;
                self.pos = 0;
                continue;
            }
            let step = available.min(count);
            self.pos += step;
            count -= step;
        }
        Some(())
    }

    /// Lee `count` caracteres. Si el texto continua en el trozo siguiente,
    /// ese trozo empieza con un byte que vuelve a indicar si los caracteres
    /// son de 1 o 2 bytes.
    fn chars(&mut self, count: usize, mut wide: bool) -> Option<String> {
        let mut out = String::with_capacity(count.min(4096));
        let mut remaining = count;
        while remaining > 0 {
            let chunk = self.chunks.get(self.index)?;
            if self.pos >= chunk.len() {
                self.index += 1;
                self.pos = 0;
                let next = self.chunks.get(self.index)?;
                wide = next.first()? & 1 != 0;
                self.pos = 1;
                continue;
            }
            if wide {
                let lo = chunk[self.pos];
                let hi = *chunk.get(self.pos + 1)?;
                self.pos += 2;
                out.push(
                    char::from_u32(u32::from(u16::from_le_bytes([lo, hi]))).unwrap_or('\u{FFFD}'),
                );
            } else {
                out.push(char::from(chunk[self.pos]));
                self.pos += 1;
            }
            remaining -= 1;
        }
        Some(out)
    }

    /// Texto con formato de Excel (XLUnicodeRichExtendedString).
    fn rich_string(&mut self) -> Option<String> {
        let count = usize::from(self.u16()?);
        let flags = self.byte()?;
        let runs = if flags & 0x08 != 0 {
            usize::from(self.u16()?)
        } else {
            0
        };
        let ext = if flags & 0x04 != 0 {
            self.u32()? as usize
        } else {
            0
        };
        let text = self.chars(count, flags & 0x01 != 0)?;
        self.skip(runs * 4)?;
        self.skip(ext)?;
        Some(text)
    }
}

/// Texto simple de Excel (XLUnicodeString): largo de 16 bits.
fn biff_string(data: &[u8], pos: usize) -> Option<String> {
    let count = usize::from(u16_at(data, pos)?);
    let flags = *data.get(pos + 2)?;
    let start = pos + 3;
    if flags & 1 != 0 {
        Some(utf16le(data.get(start..start + count * 2)?))
    } else {
        Some(
            data.get(start..start + count)?
                .iter()
                .map(|&b| char::from(b))
                .collect(),
        )
    }
}

/// Numero en formato RK (comprimido) de Excel.
fn rk_value(rk: u32) -> f64 {
    let value = if rk & 0x02 != 0 {
        f64::from((rk as i32) >> 2)
    } else {
        f64::from_bits(u64::from(rk & 0xFFFF_FFFC) << 32)
    };
    if rk & 0x01 != 0 {
        value / 100.0
    } else {
        value
    }
}

/// Muestra un numero como en las hojas XLSX: enteros sin decimales.
fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn excel(compound: &Compound, units: &mut Vec<Unit>) -> io::Result<()> {
    let Some(stream) = compound.stream("Workbook")? else {
        return Err(if compound.has_stream("Book") {
            invalid("formato de Excel 5/95 no soportado (solo Excel 97 o posterior)")
        } else {
            invalid("no es una hoja de calculo de Excel (.xls) valida")
        });
    };

    // Bloque global: textos compartidos, hojas, formatos y estilos de celda.
    let mut shared: Vec<String> = Vec::new();
    let mut sheets: Vec<(String, usize)> = Vec::new();
    let mut formats: HashMap<u16, NumberKind> = HashMap::new();
    let mut cell_styles: Vec<NumberKind> = Vec::new();
    let mut xf_formats: Vec<u16> = Vec::new();
    let mut date1904 = false;
    for record in biff_records(&stream, 0) {
        let data = record.data();
        match record.kind {
            0x002F => return Err(invalid("documento protegido con contrasena")),
            0x0022 => date1904 = u16_at(data, 0) == Some(1),
            0x0085 => {
                // BOUNDSHEET: posicion de la hoja, tipo y nombre.
                let position = u32_at(data, 0).unwrap_or(0) as usize;
                let sheet_type = data.get(5).copied().unwrap_or(0);
                let count = usize::from(data.get(6).copied().unwrap_or(0));
                let wide = data.get(7).copied().unwrap_or(0) & 1 != 0;
                let name = if wide {
                    utf16le(data.get(8..8 + count * 2).unwrap_or(&[]))
                } else {
                    data.get(8..8 + count)
                        .unwrap_or(&[])
                        .iter()
                        .map(|&b| char::from(b))
                        .collect()
                };
                if sheet_type == 0 {
                    sheets.push((name, position));
                }
            }
            0x041E => {
                // FORMAT: codigo de formato de numero.
                if let (Some(id), Some(code)) = (u16_at(data, 0), biff_string(data, 2)) {
                    formats.insert(id, classify_format(&code));
                }
            }
            0x00E0 => xf_formats.push(u16_at(data, 2).unwrap_or(0)),
            0x00FC => {
                // SST: tabla de textos compartidos.
                let mut reader = Chunks::new(&record.chunks, 8);
                let unique = u32_at(data, 4).unwrap_or(0) as usize;
                for _ in 0..unique.min(10_000_000) {
                    match reader.rich_string() {
                        Some(text) => shared.push(text),
                        None => break,
                    }
                }
            }
            _ => {}
        }
    }
    for id in xf_formats {
        cell_styles.push(match id {
            14..=17 | 27..=31 | 34..=36 | 50..=58 => NumberKind::Date,
            18..=21 | 32 | 33 | 45..=47 => NumberKind::Time,
            22 => NumberKind::DateTime,
            _ => formats.get(&id).copied().unwrap_or(NumberKind::Plain),
        });
    }

    let show_number = |value: f64, style: u16| -> String {
        match cell_styles
            .get(usize::from(style))
            .copied()
            .unwrap_or(NumberKind::Plain)
        {
            NumberKind::Plain => number_text(value),
            kind => format_serial(&value.to_string(), kind, date1904)
                .unwrap_or_else(|| number_text(value)),
        }
    };

    for (name, position) in sheets {
        let mut rows: BTreeMap<u16, BTreeMap<u16, String>> = BTreeMap::new();
        let mut pending_formula: Option<(u16, u16)> = None;
        let mut put = |row: u16, col: u16, text: String| {
            if !text.trim().is_empty() {
                rows.entry(row).or_default().insert(col, text);
            }
        };
        for record in biff_records(&stream, position).iter().skip(1) {
            let data = record.data();
            let (Some(row), Some(col), Some(style)) =
                (u16_at(data, 0), u16_at(data, 2), u16_at(data, 4))
            else {
                continue;
            };
            match record.kind {
                0x00FD => {
                    // LABELSST: texto compartido.
                    let index = u32_at(data, 6).unwrap_or(u32::MAX) as usize;
                    if let Some(text) = shared.get(index) {
                        put(row, col, text.clone());
                    }
                }
                0x0204 => {
                    if let Some(text) = biff_string(data, 6) {
                        put(row, col, text);
                    }
                }
                0x0203 => {
                    if let Some(bytes) = data.get(6..14) {
                        let value = f64::from_le_bytes(bytes.try_into().unwrap_or([0; 8]));
                        put(row, col, show_number(value, style));
                    }
                }
                0x027E => {
                    if let Some(rk) = u32_at(data, 6) {
                        put(row, col, show_number(rk_value(rk), style));
                    }
                }
                0x00BD => {
                    // MULRK: varios numeros seguidos en la misma fila.
                    let mut pos = 4;
                    let mut column = col;
                    while pos + 6 <= data.len().saturating_sub(2) {
                        let style = u16_at(data, pos).unwrap_or(0);
                        let rk = u32_at(data, pos + 2).unwrap_or(0);
                        put(row, column, show_number(rk_value(rk), style));
                        column = column.wrapping_add(1);
                        pos += 6;
                    }
                }
                0x0205 => {
                    // BOOLERR: logico o error.
                    if data.get(7) == Some(&0) {
                        let value = data.get(6) == Some(&1);
                        put(
                            row,
                            col,
                            String::from(if value { "VERDADERO" } else { "FALSO" }),
                        );
                    }
                }
                0x0006 => {
                    // FORMULA: se muestra el ultimo resultado calculado.
                    let Some(result) = data.get(6..14) else {
                        continue;
                    };
                    if result[6] == 0xFF && result[7] == 0xFF {
                        match result[0] {
                            // El texto del resultado viene en el registro STRING.
                            0 => pending_formula = Some((row, col)),
                            1 => put(
                                row,
                                col,
                                String::from(if result[2] == 1 { "VERDADERO" } else { "FALSO" }),
                            ),
                            _ => {}
                        }
                    } else {
                        let value = f64::from_le_bytes(result.try_into().unwrap_or([0; 8]));
                        put(row, col, show_number(value, style));
                    }
                }
                _ => {}
            }
        }
        // Los registros STRING no tienen fila/columna: se procesan aparte.
        for record in biff_records(&stream, position).iter().skip(1) {
            match record.kind {
                0x0006 => {
                    let data = record.data();
                    let is_text =
                        data.get(12..14) == Some(&[0xFF, 0xFF]) && data.get(6) == Some(&0);
                    pending_formula = if is_text {
                        u16_at(data, 0).zip(u16_at(data, 2))
                    } else {
                        None
                    };
                }
                0x0207 => {
                    if let Some((row, col)) = pending_formula.take() {
                        let mut reader = Chunks::new(&record.chunks, 0);
                        let count = reader.u16().map(usize::from);
                        let flags = reader.byte();
                        if let (Some(count), Some(flags)) = (count, flags) {
                            if let Some(text) = reader.chars(count, flags & 1 != 0) {
                                put(row, col, text);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (row, cells) in rows {
            units.push(Unit {
                location: format!("hoja '{name}', fila {}", u32::from(row) + 1),
                text: cells.into_values().collect::<Vec<_>>().join(" | "),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// PowerPoint 97-2003 (.ppt)
// ---------------------------------------------------------------------------

const RT_DOCUMENT: u16 = 0x03E8;
const RT_SLIDE: u16 = 0x03EE;
const RT_NOTES: u16 = 0x03F0;
const RT_NOTES_ATOM: u16 = 0x03F1;
const RT_SLIDE_PERSIST: u16 = 0x03F3;
const RT_SLIDE_LIST: u16 = 0x0FF0;
const RT_TEXT_CHARS: u16 = 0x0FA0;
const RT_TEXT_BYTES: u16 = 0x0FA8;
const RT_PERSIST_DIRECTORY: u16 = 0x1772;
const RT_CRYPT_SESSION: u16 = 0x2F14;

/// Cabecera de un registro de PowerPoint: (es contenedor, instancia, tipo,
/// inicio de los datos, fin de los datos).
fn ppt_header(data: &[u8], pos: usize) -> Option<(bool, u16, u16, usize, usize)> {
    let ver_instance = u16_at(data, pos)?;
    let kind = u16_at(data, pos + 2)?;
    let len = u32_at(data, pos + 4)? as usize;
    let start = pos + 8;
    let end = start.checked_add(len)?.min(data.len());
    Some((
        ver_instance & 0x0F == 0x0F,
        ver_instance >> 4,
        kind,
        start,
        end,
    ))
}

/// Elemento pendiente al recorrer el arbol de registros.
enum Item {
    Range(usize, usize),
    Text(String),
}

/// Recorre (sin recursion) todos los registros entre `start` y `end` y
/// devuelve, en orden, el texto de los atomos de texto que contengan.
fn ppt_texts(data: &[u8], start: usize, end: usize) -> Vec<String> {
    let mut texts = Vec::new();
    let mut stack = vec![Item::Range(start, end)];
    // Tope de registros visitados: un archivo malicioso con contenedores
    // anidados no puede hacer que el recorrido sea interminable.
    let mut budget = 5_000_000usize;
    while let Some(item) = stack.pop() {
        let (mut pos, end) = match item {
            Item::Text(text) => {
                texts.push(text);
                continue;
            }
            Item::Range(pos, end) => (pos, end),
        };
        let mut children = Vec::new();
        while pos + 8 <= end && budget > 0 {
            budget -= 1;
            let Some((container, _, kind, data_start, data_end)) = ppt_header(data, pos) else {
                break;
            };
            let body = data.get(data_start..data_end).unwrap_or(&[]);
            match kind {
                RT_TEXT_CHARS => children.push(Item::Text(utf16le(body))),
                RT_TEXT_BYTES => {
                    children.push(Item::Text(body.iter().map(|&b| cp1252_char(b)).collect()))
                }
                _ if container => children.push(Item::Range(data_start, data_end)),
                _ => {}
            }
            pos = data_end.max(pos + 8);
        }
        // Se apilan al reves para procesarlos en orden.
        stack.extend(children.into_iter().rev());
    }
    texts
}

/// Separa el texto de un atomo en parrafos (`\r` separa parrafos y el
/// caracter 0x0B es un salto de linea dentro del parrafo).
fn ppt_paragraphs(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(['\r', '\n']).filter_map(|paragraph| {
        let clean: String = paragraph
            .chars()
            .map(|c| if c == '\u{0B}' { ' ' } else { c })
            .filter(|&c| c == '\t' || (c as u32) >= 0x20)
            .collect();
        let clean = clean.trim();
        (!clean.is_empty()).then(|| clean.to_string())
    })
}

/// Registros hijos directos de un contenedor: (instancia, tipo, inicio, fin).
fn ppt_children(data: &[u8], start: usize, end: usize) -> Vec<(u16, u16, usize, usize)> {
    let mut children = Vec::new();
    let mut pos = start;
    while pos + 8 <= end && children.len() < 1_000_000 {
        let Some((_, instance, kind, data_start, data_end)) = ppt_header(data, pos) else {
            break;
        };
        children.push((instance, kind, data_start, data_end));
        pos = data_end.max(pos + 8);
    }
    children
}

fn powerpoint(compound: &Compound, units: &mut Vec<Unit>) -> io::Result<()> {
    let stream = compound
        .stream("PowerPoint Document")?
        .ok_or_else(|| invalid("no es una presentacion de PowerPoint (.ppt) valida"))?;

    // Nivel superior del flujo: el contenedor del documento y los
    // directorios de persistencia (identificador -> posicion en el flujo).
    // Al guardar de forma incremental se agregan nuevos al final, que
    // reemplazan a los anteriores.
    let mut persist: HashMap<u32, usize> = HashMap::new();
    let mut document: Option<(usize, usize)> = None;
    for (_, kind, start, end) in ppt_children(&stream, 0, stream.len()) {
        match kind {
            RT_DOCUMENT => document = Some((start, end)),
            RT_CRYPT_SESSION => return Err(invalid("documento protegido con contrasena")),
            RT_PERSIST_DIRECTORY => {
                let mut pos = start;
                while pos + 4 <= end {
                    let header = u32_at(&stream, pos).unwrap_or(0);
                    let first_id = header & 0x000F_FFFF;
                    let count = (header >> 20) as usize;
                    pos += 4;
                    for k in 0..count {
                        if let Some(offset) = u32_at(&stream, pos) {
                            persist.insert(first_id.wrapping_add(k as u32), offset as usize);
                        }
                        pos += 4;
                    }
                }
            }
            _ => {}
        }
    }

    // Listas de diapositivas (instancia 0) y de notas (instancia 2). Cada
    // SlidePersistAtom abre una entrada; los atomos de texto que le siguen
    // son el texto de sus marcadores de posicion (titulo, cuerpo...).
    struct Entry {
        persist_id: u32,
        slide_id: u32,
        texts: Vec<String>,
    }
    let mut slides: Vec<Entry> = Vec::new();
    let mut notes: Vec<Entry> = Vec::new();
    if let Some((start, end)) = document {
        for (instance, kind, list_start, list_end) in ppt_children(&stream, start, end) {
            if kind != RT_SLIDE_LIST || (instance != 0 && instance != 2) {
                continue;
            }
            let list = if instance == 0 {
                &mut slides
            } else {
                &mut notes
            };
            for (_, kind, atom_start, atom_end) in ppt_children(&stream, list_start, list_end) {
                let body = stream.get(atom_start..atom_end).unwrap_or(&[]);
                match kind {
                    RT_SLIDE_PERSIST => list.push(Entry {
                        persist_id: u32_at(body, 0).unwrap_or(u32::MAX),
                        slide_id: u32_at(body, 12).unwrap_or(0),
                        texts: Vec::new(),
                    }),
                    RT_TEXT_CHARS | RT_TEXT_BYTES => {
                        if let Some(entry) = list.last_mut() {
                            entry
                                .texts
                                .extend(ppt_texts(&stream, atom_start - 8, atom_end));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Contenedor (diapositiva o notas) al que apunta un identificador de
    // persistencia, si es del tipo esperado.
    let container = |persist_id: u32, expected: u16| -> Option<(usize, usize)> {
        let offset = *persist.get(&persist_id)?;
        let (_, _, kind, start, end) = ppt_header(&stream, offset)?;
        (kind == expected).then_some((start, end))
    };

    let mut slide_numbers: HashMap<u32, usize> = HashMap::new();
    for (index, slide) in slides.iter().enumerate() {
        let number = index + 1;
        slide_numbers.insert(slide.slide_id, number);
        let mut texts = slide.texts.clone();
        // Texto de las formas que no son marcadores (cuadros de texto...).
        if let Some((start, end)) = container(slide.persist_id, RT_SLIDE) {
            texts.extend(ppt_texts(&stream, start, end));
        }
        for text in &texts {
            for paragraph in ppt_paragraphs(text) {
                units.push(Unit {
                    location: format!("diapositiva {number}"),
                    text: paragraph,
                });
            }
        }
    }

    for entry in &notes {
        let Some((start, end)) = container(entry.persist_id, RT_NOTES) else {
            continue;
        };
        // NotesAtom indica a que diapositiva pertenecen las notas.
        let slide = ppt_children(&stream, start, end)
            .into_iter()
            .find(|(_, kind, _, _)| *kind == RT_NOTES_ATOM)
            .and_then(|(_, _, atom_start, _)| u32_at(&stream, atom_start))
            .and_then(|slide_id| slide_numbers.get(&slide_id));
        let location = match slide {
            Some(number) => format!("notas diapositiva {number}"),
            None => String::from("notas"),
        };
        let mut texts = entry.texts.clone();
        texts.extend(ppt_texts(&stream, start, end));
        for text in &texts {
            for paragraph in ppt_paragraphs(text) {
                units.push(Unit {
                    location: location.clone(),
                    text: paragraph,
                });
            }
        }
    }

    // Sin estructura reconocible: todo el texto del flujo, sin ubicacion.
    if slides.is_empty() {
        for text in ppt_texts(&stream, 0, stream.len()) {
            for paragraph in ppt_paragraphs(&text) {
                units.push(Unit {
                    location: String::from("presentacion"),
                    text: paragraph,
                });
            }
        }
    }
    Ok(())
}
