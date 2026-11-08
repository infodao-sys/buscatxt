//! Extraccion de texto de archivos PDF.
//!
//! Un PDF es una coleccion de "objetos" numerados (diccionarios, listas,
//! textos, flujos de datos). El texto de cada pagina esta en uno o varios
//! flujos de contenido, normalmente comprimidos, como una secuencia de
//! operadores de dibujo: `/F1 12 Tf` (elegir fuente), `(Hola) Tj` (escribir
//! texto), `0 -14 Td` (moverse a la linea siguiente)...
//!
//! Pasos:
//! 1. Se recorre el archivo buscando los objetos `N 0 obj ... endobj`, sin
//!    depender de la tabla de referencias cruzadas (asi tambien funciona con
//!    PDF algo danados). Se expanden los "flujos de objetos" (`/ObjStm`)
//!    que usan los PDF modernos para guardar objetos comprimidos.
//! 2. Se recorre el arbol de paginas desde el catalogo (`/Root`).
//! 3. Para cada pagina se interpretan los operadores de texto y se traducen
//!    los codigos de cada fuente a Unicode: con su tabla `/ToUnicode` si la
//!    tiene, o con su codificacion (`WinAnsiEncoding`, `MacRomanEncoding`,
//!    `/Differences`).
//!
//! Limitaciones conocidas: no se lee el texto de PDF escaneados (son
//! imagenes), de PDF cifrados, ni de fuentes sin tabla `/ToUnicode` que usan
//! codigos propios. Los espacios entre palabras se deducen de forma
//! aproximada.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::rc::Rc;

use super::inflate::{zlib, InflateError};
use super::{invalid, Unit, MAX_DOC_BYTES, MAX_PART_BYTES};

/// Tamano maximo de un PDF que se carga en memoria.
const MAX_PDF_BYTES: u64 = 512 * 1024 * 1024;
/// Profundidad maxima de listas y diccionarios anidados.
const MAX_DEPTH: usize = 64;
/// Cantidad maxima de paginas que se procesan.
const MAX_PAGES: usize = 100_000;
/// Profundidad maxima de formularios (`Do`) dentro de formularios.
const MAX_FORM_DEPTH: usize = 8;
/// Entradas maximas de una tabla ToUnicode.
const MAX_CMAP_ENTRIES: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Objetos y lectura de la sintaxis PDF
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct Dict(Vec<(String, Obj)>);

impl Dict {
    fn get(&self, key: &str) -> Option<&Obj> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value)
    }
}

#[derive(Clone, Debug)]
enum Obj {
    Null,
    Bool(#[allow(dead_code)] bool),
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    Array(Vec<Obj>),
    Dict(Dict),
    /// Referencia a otro objeto: `12 0 R`.
    Ref(u32),
    /// Diccionario y posicion (inicio, fin) de los datos en el archivo.
    Stream(Dict, usize, usize),
    /// Palabra clave desconocida fuera de su lugar (se conserva como valor
    /// vacio para no desalinear los operandos).
    Op,
}

static NULL: Obj = Obj::Null;

impl Obj {
    fn as_name(&self) -> Option<&str> {
        match self {
            Obj::Name(name) => Some(name),
            _ => None,
        }
    }
    fn as_num(&self) -> Option<f64> {
        match self {
            Obj::Num(n) => Some(*n),
            _ => None,
        }
    }
}

enum Tok<'a> {
    Num(f64, bool),
    Str(Vec<u8>),
    Name(String),
    ArrayOpen,
    ArrayClose,
    DictOpen,
    DictClose,
    Word(&'a [u8]),
}

fn is_space(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Analizador lexico de la sintaxis PDF (tambien sirve para los flujos de
/// contenido y las tablas ToUnicode, que usan la misma sintaxis).
struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Lexer { data, pos }
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while let Some(byte) = self.peek() {
            if is_space(byte) {
                self.pos += 1;
            } else if byte == b'%' {
                while let Some(byte) = self.peek() {
                    if byte == b'\n' || byte == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn token(&mut self) -> Option<Tok<'a>> {
        loop {
            self.skip_space();
            let byte = self.peek()?;
            match byte {
                b'(' => return Some(Tok::Str(self.literal_string())),
                b'<' if self.data.get(self.pos + 1) == Some(&b'<') => {
                    self.pos += 2;
                    return Some(Tok::DictOpen);
                }
                b'<' => return Some(Tok::Str(self.hex_string())),
                b'>' if self.data.get(self.pos + 1) == Some(&b'>') => {
                    self.pos += 2;
                    return Some(Tok::DictClose);
                }
                b'[' => {
                    self.pos += 1;
                    return Some(Tok::ArrayOpen);
                }
                b']' => {
                    self.pos += 1;
                    return Some(Tok::ArrayClose);
                }
                b'/' => return Some(Tok::Name(self.name())),
                b'0'..=b'9' | b'+' | b'-' | b'.' => return Some(self.number()),
                b'{' | b'}' => {
                    self.pos += 1;
                    return Some(Tok::Word(&self.data[self.pos - 1..self.pos]));
                }
                // Delimitadores sueltos (archivo danado): se saltan.
                b')' | b'>' => self.pos += 1,
                _ => {
                    let start = self.pos;
                    while let Some(byte) = self.peek() {
                        if is_space(byte) || is_delimiter(byte) {
                            break;
                        }
                        self.pos += 1;
                    }
                    return Some(Tok::Word(&self.data[start..self.pos]));
                }
            }
        }
    }

    fn number(&mut self) -> Tok<'a> {
        let start = self.pos;
        while let Some(byte) = self.peek() {
            if byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.') {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text = std::str::from_utf8(&self.data[start..self.pos]).unwrap_or("0");
        let integer = !text.contains('.');
        Tok::Num(text.parse().unwrap_or(0.0), integer)
    }

    fn name(&mut self) -> String {
        self.pos += 1;
        let mut name = Vec::new();
        while let Some(byte) = self.peek() {
            if is_space(byte) || is_delimiter(byte) {
                break;
            }
            self.pos += 1;
            if byte == b'#' {
                let high = self.data.get(self.pos).copied().and_then(hex_value);
                let low = self.data.get(self.pos + 1).copied().and_then(hex_value);
                if let (Some(high), Some(low)) = (high, low) {
                    name.push(high << 4 | low);
                    self.pos += 2;
                    continue;
                }
            }
            name.push(byte);
        }
        String::from_utf8_lossy(&name).into_owned()
    }

    fn literal_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            match byte {
                b'(' => {
                    depth += 1;
                    out.push(byte);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(byte);
                }
                b'\\' => {
                    let Some(next) = self.peek() else { break };
                    self.pos += 1;
                    match next {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut value = u32::from(next - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(digit @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(digit - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(value as u8);
                        }
                        // Barra al final de la linea: continuacion.
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                _ => out.push(byte),
            }
        }
        out
    }

    fn hex_string(&mut self) -> Vec<u8> {
        self.pos += 1;
        let mut out = Vec::new();
        let mut high: Option<u8> = None;
        while let Some(byte) = self.peek() {
            self.pos += 1;
            if byte == b'>' {
                break;
            }
            if let Some(value) = hex_value(byte) {
                match high.take() {
                    Some(h) => out.push(h << 4 | value),
                    None => high = Some(value),
                }
            }
        }
        if let Some(h) = high {
            out.push(h << 4);
        }
        out
    }

    fn value(&mut self, depth: usize) -> Option<Obj> {
        let token = self.token()?;
        self.value_from(token, depth)
    }

    /// Construye un objeto completo a partir de su primer token.
    fn value_from(&mut self, token: Tok<'a>, depth: usize) -> Option<Obj> {
        if depth > MAX_DEPTH {
            return None;
        }
        Some(match token {
            Tok::Num(n, true) if n >= 0.0 => {
                // Posible referencia "N G R".
                let save = self.pos;
                if let Some(Tok::Num(_, true)) = self.token() {
                    if let Some(Tok::Word(b"R")) = self.token() {
                        return Some(Obj::Ref(n as u32));
                    }
                }
                self.pos = save;
                Obj::Num(n)
            }
            Tok::Num(n, _) => Obj::Num(n),
            Tok::Str(s) => Obj::Str(s),
            Tok::Name(name) => Obj::Name(name),
            Tok::ArrayOpen => {
                let mut items = Vec::new();
                loop {
                    match self.token()? {
                        Tok::ArrayClose => break,
                        token => items.push(self.value_from(token, depth + 1)?),
                    }
                }
                Obj::Array(items)
            }
            Tok::DictOpen => {
                let mut entries = Vec::new();
                loop {
                    match self.token()? {
                        Tok::DictClose => break,
                        Tok::Name(key) => {
                            let token = self.token()?;
                            if matches!(token, Tok::DictClose) {
                                entries.push((key, Obj::Null));
                                break;
                            }
                            entries.push((key, self.value_from(token, depth + 1)?));
                        }
                        _ => {} // basura: se ignora
                    }
                }
                Obj::Dict(Dict(entries))
            }
            Tok::ArrayClose | Tok::DictClose => Obj::Null,
            Tok::Word(b"true") => Obj::Bool(true),
            Tok::Word(b"false") => Obj::Bool(false),
            Tok::Word(b"null") => Obj::Null,
            Tok::Word(_) => Obj::Op,
        })
    }
}

fn find(data: &[u8], pattern: &[u8], from: usize) -> Option<usize> {
    let first = *pattern.first()?;
    let mut pos = from;
    while pos + pattern.len() <= data.len() {
        let index = data[pos..].iter().position(|&byte| byte == first)?;
        let candidate = pos + index;
        if data.get(candidate..candidate + pattern.len()) == Some(pattern) {
            return Some(candidate);
        }
        pos = candidate + 1;
    }
    None
}

/// Comprueba que `obj` en la posicion `at` va precedido de "N G" y devuelve N.
fn object_number(data: &[u8], at: usize) -> Option<u32> {
    let mut pos = at;
    let skip_space_back = |pos: &mut usize| {
        let start = *pos;
        while *pos > 0 && is_space(data[*pos - 1]) {
            *pos -= 1;
        }
        *pos < start
    };
    let digits_back = |pos: &mut usize| {
        let end = *pos;
        while *pos > 0 && data[*pos - 1].is_ascii_digit() {
            *pos -= 1;
        }
        (*pos < end).then_some((*pos, end))
    };
    if !skip_space_back(&mut pos) {
        return None;
    }
    digits_back(&mut pos)?;
    if !skip_space_back(&mut pos) {
        return None;
    }
    let (start, end) = digits_back(&mut pos)?;
    if start > 0 && !(is_space(data[start - 1]) || is_delimiter(data[start - 1])) {
        return None;
    }
    std::str::from_utf8(&data[start..end]).ok()?.parse().ok()
}

// ---------------------------------------------------------------------------
// Documento
// ---------------------------------------------------------------------------

struct Pdf<'a> {
    data: &'a [u8],
    objects: HashMap<u32, Obj>,
    root: Option<Obj>,
    encrypted: bool,
    /// Bytes descomprimidos que todavia se permiten para este documento.
    budget: Cell<usize>,
}

impl<'a> Pdf<'a> {
    fn parse(data: &'a [u8]) -> Pdf<'a> {
        let mut pdf = Pdf {
            data,
            objects: HashMap::new(),
            root: None,
            encrypted: false,
            budget: Cell::new(MAX_DOC_BYTES),
        };
        pdf.scan_objects();
        pdf.scan_trailers();
        pdf.expand_object_streams();
        pdf
    }

    /// Busca todos los `N G obj` del archivo. Si un objeto aparece varias
    /// veces (PDF modificados), vale la ultima version.
    fn scan_objects(&mut self) {
        let data = self.data;
        let mut pos = 0;
        while let Some(at) = find(data, b"obj", pos) {
            pos = at + 3;
            if data
                .get(at + 3)
                .is_some_and(|&b| !is_space(b) && !is_delimiter(b))
            {
                continue;
            }
            let Some(number) = object_number(data, at) else {
                continue;
            };
            let mut lexer = Lexer::new(data, at + 3);
            let Some(value) = lexer.value(0) else {
                continue;
            };
            lexer.skip_space();
            if !data[lexer.pos..].starts_with(b"stream") {
                self.check_trailer_dict(&value);
                self.objects.insert(number, value);
                pos = lexer.pos.max(pos);
                continue;
            }

            let mut start = lexer.pos + 6;
            if data.get(start) == Some(&b'\r') {
                start += 1;
            }
            if data.get(start) == Some(&b'\n') {
                start += 1;
            }
            let dict = match value {
                Obj::Dict(dict) => dict,
                _ => Dict::default(),
            };
            let declared = dict
                .get("Length")
                .and_then(Obj::as_num)
                .filter(|&len| len >= 0.0)
                .map(|len| len as usize);
            let end = match declared {
                Some(len)
                    if start.checked_add(len).is_some_and(|end| end <= data.len()) && {
                        let mut after = Lexer::new(data, start + len);
                        after.skip_space();
                        data[after.pos..].starts_with(b"endstream")
                    } =>
                {
                    start + len
                }
                _ => match find(data, b"endstream", start) {
                    Some(mut end) => {
                        if end > start && data[end - 1] == b'\n' {
                            end -= 1;
                        }
                        if end > start && data[end - 1] == b'\r' {
                            end -= 1;
                        }
                        end
                    }
                    None => data.len(),
                },
            };
            let stream = Obj::Stream(dict, start, end);
            self.check_trailer_dict(&stream);
            self.objects.insert(number, stream);
            pos = end;
        }
    }

    /// Los diccionarios `trailer` (y los flujos `/XRef`) indican el catalogo
    /// (`/Root`) y si el documento esta cifrado (`/Encrypt`).
    fn scan_trailers(&mut self) {
        let mut pos = 0;
        while let Some(at) = find(self.data, b"trailer", pos) {
            pos = at + 7;
            if let Some(value) = Lexer::new(self.data, pos).value(0) {
                self.check_trailer_dict(&value);
            }
        }
    }

    fn check_trailer_dict(&mut self, value: &Obj) {
        let dict = match value {
            Obj::Dict(dict) => dict,
            Obj::Stream(dict, ..) if dict.get("Type").and_then(Obj::as_name) == Some("XRef") => {
                dict
            }
            _ => return,
        };
        let is_trailer = dict.get("Root").is_some() && dict.get("Size").is_some();
        if !is_trailer {
            return;
        }
        if let Some(root) = dict.get("Root") {
            self.root = Some(root.clone());
        }
        if dict.get("Encrypt").is_some() {
            self.encrypted = true;
        }
    }

    /// Los PDF 1.5+ guardan muchos objetos comprimidos dentro de un flujo
    /// `/Type /ObjStm`: `N` pares (numero, desplazamiento) y luego los objetos.
    fn expand_object_streams(&mut self) {
        let mut found: Vec<(u32, Obj)> = Vec::new();
        for obj in self.objects.values() {
            let Obj::Stream(dict, start, end) = obj else {
                continue;
            };
            if dict.get("Type").and_then(Obj::as_name) != Some("ObjStm") {
                continue;
            }
            let Some(data) = self.decode_stream(dict, *start, *end) else {
                continue;
            };
            let count = self.number(dict.get("N")).unwrap_or(0.0).max(0.0) as usize;
            let first = self.number(dict.get("First")).unwrap_or(0.0).max(0.0) as usize;
            let mut lexer = Lexer::new(&data, 0);
            let mut pairs = Vec::new();
            for _ in 0..count.min(1_000_000) {
                let (Some(Tok::Num(number, _)), Some(Tok::Num(offset, _))) =
                    (lexer.token(), lexer.token())
                else {
                    break;
                };
                pairs.push((number as u32, offset as usize));
            }
            for (number, offset) in pairs {
                let Some(pos) = first.checked_add(offset).filter(|&p| p < data.len()) else {
                    continue;
                };
                if let Some(value) = Lexer::new(&data, pos).value(0) {
                    found.push((number, value));
                }
            }
        }
        for (number, value) in found {
            self.check_trailer_dict(&value);
            self.objects.entry(number).or_insert(value);
        }
    }

    /// Sigue las referencias `N 0 R` hasta llegar a un objeto directo.
    fn resolve<'b>(&'b self, obj: &'b Obj) -> &'b Obj {
        let mut current = obj;
        for _ in 0..32 {
            match current {
                Obj::Ref(number) => current = self.objects.get(number).unwrap_or(&NULL),
                _ => return current,
            }
        }
        &NULL
    }

    fn dict<'b>(&'b self, obj: Option<&'b Obj>) -> Option<&'b Dict> {
        match self.resolve(obj?) {
            Obj::Dict(dict) | Obj::Stream(dict, ..) => Some(dict),
            _ => None,
        }
    }

    fn number(&self, obj: Option<&Obj>) -> Option<f64> {
        self.resolve(obj?).as_num()
    }

    fn name<'b>(&'b self, obj: Option<&'b Obj>) -> Option<&'b str> {
        self.resolve(obj?).as_name()
    }

    /// Datos decodificados de un flujo, aplicando sus filtros.
    fn stream_data(&self, obj: Option<&Obj>) -> Option<Vec<u8>> {
        match self.resolve(obj?) {
            Obj::Stream(dict, start, end) => self.decode_stream(dict, *start, *end),
            _ => None,
        }
    }

    fn decode_stream(&self, dict: &Dict, start: usize, end: usize) -> Option<Vec<u8>> {
        let raw = self.data.get(start..end.max(start))?;
        let filters: Vec<&str> = match self.resolve(dict.get("Filter").unwrap_or(&NULL)) {
            Obj::Name(name) => vec![name.as_str()],
            Obj::Array(items) => items
                .iter()
                .filter_map(|item| self.resolve(item).as_name())
                .collect(),
            _ => Vec::new(),
        };
        let params: Vec<Option<&Dict>> =
            match self.resolve(dict.get("DecodeParms").unwrap_or(&NULL)) {
                Obj::Array(items) => items.iter().map(|item| self.dict(Some(item))).collect(),
                Obj::Dict(params) => vec![Some(params)],
                _ => Vec::new(),
            };

        let mut data = raw.to_vec();
        for (index, filter) in filters.iter().enumerate() {
            data = match *filter {
                "FlateDecode" | "Fl" => {
                    let limit = MAX_PART_BYTES.min(self.budget.get());
                    let (out, error) = zlib(&data, limit);
                    if error == Some(InflateError::TooLarge) {
                        return None;
                    }
                    self.budget.set(self.budget.get().saturating_sub(out.len()));
                    match params.get(index).copied().flatten() {
                        Some(params) => self.unpredict(out, params),
                        None => out,
                    }
                }
                "ASCIIHexDecode" | "AHx" => {
                    // Mismo formato que un texto hexadecimal `<...>`.
                    let mut wrapped = Vec::with_capacity(data.len() + 1);
                    wrapped.push(b'<');
                    wrapped.extend_from_slice(&data);
                    Lexer::new(&wrapped, 0).hex_string()
                }
                "ASCII85Decode" | "A85" => ascii85(&data),
                // Filtros de imagen u otros no soportados: no hay texto.
                _ => return None,
            };
        }
        Some(data)
    }

    /// Deshace los predictores PNG (`/Predictor 10..15`) que algunos
    /// generadores aplican antes de comprimir.
    fn unpredict(&self, data: Vec<u8>, params: &Dict) -> Vec<u8> {
        let predictor = self.number(params.get("Predictor")).unwrap_or(1.0) as i64;
        if predictor < 10 {
            return data;
        }
        let colors = self
            .number(params.get("Colors"))
            .unwrap_or(1.0)
            .clamp(1.0, 32.0) as usize;
        let bits = self
            .number(params.get("BitsPerComponent"))
            .unwrap_or(8.0)
            .clamp(1.0, 16.0) as usize;
        let columns = self
            .number(params.get("Columns"))
            .unwrap_or(1.0)
            .clamp(1.0, 1_000_000.0) as usize;
        let bpp = (colors * bits).div_ceil(8).max(1);
        let row_len = (columns * colors * bits).div_ceil(8);
        let mut out = Vec::with_capacity(data.len());
        let mut previous = vec![0u8; row_len];
        for row in data.chunks(row_len + 1) {
            let Some((&kind, row)) = row.split_first() else {
                break;
            };
            let mut current = row.to_vec();
            current.resize(row_len, 0);
            for i in 0..row_len {
                let left = if i >= bpp { current[i - bpp] } else { 0 };
                let up = previous[i];
                let up_left = if i >= bpp { previous[i - bpp] } else { 0 };
                current[i] = match kind {
                    1 => current[i].wrapping_add(left),
                    2 => current[i].wrapping_add(up),
                    3 => current[i].wrapping_add(((u16::from(left) + u16::from(up)) / 2) as u8),
                    4 => {
                        let p = i16::from(left) + i16::from(up) - i16::from(up_left);
                        let (pa, pb, pc) = (
                            (p - i16::from(left)).abs(),
                            (p - i16::from(up)).abs(),
                            (p - i16::from(up_left)).abs(),
                        );
                        let predicted = if pa <= pb && pa <= pc {
                            left
                        } else if pb <= pc {
                            up
                        } else {
                            up_left
                        };
                        current[i].wrapping_add(predicted)
                    }
                    _ => current[i],
                };
            }
            out.extend_from_slice(&current);
            previous = current;
        }
        out
    }

    /// Paginas en orden, con sus recursos (heredados del nodo padre si la
    /// pagina no los define).
    fn pages(&self) -> Vec<(&Dict, Option<&Dict>)> {
        let mut pages = Vec::new();
        let catalog = self.dict(self.root.as_ref()).or_else(|| {
            self.objects.values().find_map(|obj| match obj {
                Obj::Dict(dict) if dict.get("Type").and_then(Obj::as_name) == Some("Catalog") => {
                    Some(dict)
                }
                _ => None,
            })
        });

        if let Some(tree) = catalog.and_then(|catalog| catalog.get("Pages")) {
            let mut visited: HashSet<u32> = HashSet::new();
            let mut stack: Vec<(&Obj, Option<&Dict>)> = vec![(tree, None)];
            while let Some((node, inherited)) = stack.pop() {
                if pages.len() >= MAX_PAGES {
                    break;
                }
                if let Obj::Ref(number) = node {
                    if !visited.insert(*number) {
                        continue; // ciclo en el arbol de paginas
                    }
                }
                let Some(dict) = self.dict(Some(node)) else {
                    continue;
                };
                let resources = self.dict(dict.get("Resources")).or(inherited);
                match self.resolve(dict.get("Kids").unwrap_or(&NULL)) {
                    Obj::Array(kids) => {
                        for kid in kids.iter().rev() {
                            stack.push((kid, resources));
                        }
                    }
                    _ => pages.push((dict, resources)),
                }
            }
        }

        if pages.is_empty() {
            // Sin arbol de paginas valido: todos los objetos /Page por numero.
            let mut numbered: Vec<(&u32, &Dict)> = self
                .objects
                .iter()
                .filter_map(|(number, obj)| match obj {
                    Obj::Dict(dict) if dict.get("Type").and_then(Obj::as_name) == Some("Page") => {
                        Some((number, dict))
                    }
                    _ => None,
                })
                .collect();
            numbered.sort_by_key(|(number, _)| **number);
            pages = numbered
                .into_iter()
                .take(MAX_PAGES)
                .map(|(_, dict)| (dict, self.dict(dict.get("Resources"))))
                .collect();
        }
        pages
    }

    /// Contenido de una pagina: uno o varios flujos concatenados.
    fn page_content(&self, page: &Dict) -> Vec<u8> {
        let mut content = Vec::new();
        let Some(contents) = page.get("Contents") else {
            return content;
        };
        let parts: Vec<&Obj> = match self.resolve(contents) {
            Obj::Array(items) => items.iter().collect(),
            _ => vec![contents],
        };
        for part in parts {
            if let Some(data) = self.stream_data(Some(part)) {
                content.extend_from_slice(&data);
                content.push(b'\n');
            }
        }
        content
    }
}

fn ascii85(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut group = [0u8; 5];
    let mut count = 0;
    let mut bytes = data.iter().copied();
    if data.starts_with(b"<~") {
        bytes.next();
        bytes.next();
    }
    for byte in bytes {
        match byte {
            b'~' => break,
            b'z' if count == 0 => out.extend_from_slice(&[0, 0, 0, 0]),
            b'!'..=b'u' => {
                group[count] = byte - b'!';
                count += 1;
                if count == 5 {
                    let value = group.iter().fold(0u64, |acc, &d| acc * 85 + u64::from(d));
                    out.extend_from_slice(&(value as u32).to_be_bytes());
                    count = 0;
                }
            }
            _ => {}
        }
    }
    if count > 1 {
        for slot in group.iter_mut().skip(count) {
            *slot = 84;
        }
        let value = group.iter().fold(0u64, |acc, &d| acc * 85 + u64::from(d));
        out.extend_from_slice(&(value as u32).to_be_bytes()[..count - 1]);
    }
    out
}

// ---------------------------------------------------------------------------
// Fuentes: de codigos a Unicode
// ---------------------------------------------------------------------------

/// Tabla `/ToUnicode`: rangos de codigos validos y traduccion de cada codigo.
#[derive(Default)]
struct CMap {
    ranges: Vec<(Vec<u8>, Vec<u8>)>,
    map: HashMap<u32, String>,
}

fn code_value(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0, |acc, &b| acc << 8 | u32::from(b))
}

fn utf16be(bytes: &[u8]) -> String {
    if bytes.len() == 1 {
        return char::from(bytes[0]).to_string();
    }
    let units = bytes
        .chunks(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair.get(1).copied().unwrap_or(0)]));
    char::decode_utf16(units)
        .map(|c| c.unwrap_or('\u{FFFD}'))
        .collect()
}

fn parse_cmap(data: &[u8]) -> CMap {
    let mut cmap = CMap::default();
    let mut lexer = Lexer::new(data, 0);
    while let Some(token) = lexer.token() {
        let Tok::Word(word) = token else { continue };
        match word {
            b"begincodespacerange" => {
                while let (Some(Tok::Str(low)), Some(Tok::Str(high))) =
                    (lexer.token(), lexer.token())
                {
                    if low.len() == high.len() && !low.is_empty() {
                        cmap.ranges.push((low, high));
                    }
                }
            }
            b"beginbfchar" => {
                while let Some(Tok::Str(source)) = lexer.token() {
                    let target = match lexer.token() {
                        Some(Tok::Str(target)) => utf16be(&target),
                        Some(Tok::Name(name)) => glyph_text(&name).unwrap_or_default(),
                        _ => break,
                    };
                    if cmap.map.len() < MAX_CMAP_ENTRIES {
                        cmap.map.insert(code_value(&source), target);
                    }
                }
            }
            b"beginbfrange" => {
                while let (Some(Tok::Str(low)), Some(Tok::Str(high))) =
                    (lexer.token(), lexer.token())
                {
                    let (low, high) = (code_value(&low), code_value(&high));
                    if high < low || high - low > 0xFFFF {
                        // Rango invalido: se salta su destino.
                        let _ = lexer.value(0);
                        continue;
                    }
                    match lexer.value(0) {
                        Some(Obj::Str(target)) if !target.is_empty() => {
                            let split = target.len().saturating_sub(2);
                            let (prefix, last) = target.split_at(split);
                            let base = code_value(last);
                            for (offset, code) in (low..=high).enumerate() {
                                if cmap.map.len() >= MAX_CMAP_ENTRIES {
                                    break;
                                }
                                let value = base.wrapping_add(offset as u32);
                                let mut bytes = prefix.to_vec();
                                if last.len() == 2 {
                                    bytes.extend_from_slice(&(value as u16).to_be_bytes());
                                } else {
                                    bytes.push(value as u8);
                                }
                                cmap.map.insert(code, utf16be(&bytes));
                            }
                        }
                        Some(Obj::Array(targets)) => {
                            for (code, target) in (low..=high).zip(targets) {
                                if let Obj::Str(target) = target {
                                    if cmap.map.len() < MAX_CMAP_ENTRIES {
                                        cmap.map.insert(code, utf16be(&target));
                                    }
                                }
                            }
                        }
                        _ => break,
                    }
                }
            }
            _ => {}
        }
    }
    cmap
}

struct Font {
    /// Fuente compuesta (Type0): codigos de 2 bytes por defecto.
    composite: bool,
    cmap: Option<CMap>,
    /// Traduccion de codigos de 1 byte (fuentes simples).
    simple: Vec<String>,
    /// Anchos de los glifos de una fuente simple (en milesimas del tamano de
    /// letra), empezando por el codigo `first_char`.
    first_char: u32,
    widths: Vec<f64>,
    /// Anchos de una fuente compuesta, por codigo.
    cid_widths: HashMap<u32, f64>,
    /// Ancho de los glifos que no aparecen en las tablas anteriores.
    default_width: f64,
}

impl Font {
    /// Recorre los codigos de un texto y llama a `glyph` con el texto Unicode
    /// de cada uno, su ancho y si es el espacio de un byte (al que se aplica
    /// el espaciado entre palabras `Tw`).
    fn glyphs(&self, bytes: &[u8], mut glyph: impl FnMut(&str, f64, bool)) {
        let mut i = 0;
        while i < bytes.len() {
            let len = self.code_len(&bytes[i..]).max(1);
            let code = &bytes[i..(i + len).min(bytes.len())];
            i += len;
            let value = code_value(code);
            let text: &str =
                if let Some(text) = self.cmap.as_ref().and_then(|cmap| cmap.map.get(&value)) {
                    text
                } else if !self.composite && code.len() == 1 {
                    &self.simple[usize::from(code[0])]
                } else {
                    ""
                };
            glyph(text, self.width(value), code == [32]);
        }
    }

    fn width(&self, code: u32) -> f64 {
        let width = if self.composite {
            self.cid_widths.get(&code).copied()
        } else {
            code.checked_sub(self.first_char)
                .and_then(|index| self.widths.get(index as usize))
                .copied()
        };
        width.unwrap_or(self.default_width)
    }

    fn code_len(&self, bytes: &[u8]) -> usize {
        if let Some(cmap) = &self.cmap {
            for (low, high) in &cmap.ranges {
                let n = low.len();
                if let Some(code) = bytes.get(..n) {
                    if code >= low.as_slice() && code <= high.as_slice() {
                        return n;
                    }
                }
            }
            if let Some(shortest) = cmap.ranges.iter().map(|(low, _)| low.len()).min() {
                return shortest;
            }
        }
        if self.composite {
            2
        } else {
            1
        }
    }
}

/// Mac Roman, bytes 0x80..0xFF.
const MAC_ROMAN: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
¿¡¬√ƒ≈∆«»…\u{A0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{F8FF}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";

/// Tabla de 256 entradas para una codificacion base de fuente simple.
fn base_encoding(name: Option<&str>) -> Vec<String> {
    let mac: Vec<char> = MAC_ROMAN.chars().collect();
    (0u16..256)
        .map(|code| {
            let byte = code as u8;
            if byte < 32 {
                return String::new();
            }
            match name {
                Some("MacRomanEncoding") if byte >= 0x80 => mac
                    .get(usize::from(byte - 0x80))
                    .map(|c| c.to_string())
                    .unwrap_or_default(),
                Some("StandardEncoding") if byte == 0x27 => "’".to_string(),
                Some("StandardEncoding") if byte == 0x60 => "‘".to_string(),
                _ => crate_cp1252(byte),
            }
        })
        .collect()
}

fn crate_cp1252(byte: u8) -> String {
    // Igual que la decodificacion Windows-1252 de los archivos de texto.
    const HIGH: [u16; 32] = [
        0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022,
        0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
    ];
    let c = match byte {
        0x80..=0x9F => {
            char::from_u32(u32::from(HIGH[usize::from(byte - 0x80)])).unwrap_or('\u{FFFD}')
        }
        _ => char::from(byte),
    };
    c.to_string()
}

/// Texto de un nombre de glifo de Adobe (`/aacute` -> "á", `/uni00F1` -> "ñ",
/// `/fi` -> "fi"). Cubre los nombres habituales en textos en espanol e ingles.
fn glyph_text(name: &str) -> Option<String> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphabetic() {
            return Some(c.to_string());
        }
    }
    if let Some(hex) = name.strip_prefix("uni") {
        if hex.len() >= 4 && hex.len() % 4 == 0 {
            let units: Vec<u16> = (0..hex.len())
                .step_by(4)
                .filter_map(|i| u16::from_str_radix(hex.get(i..i + 4)?, 16).ok())
                .collect();
            return Some(
                char::decode_utf16(units)
                    .map(|c| c.unwrap_or('\u{FFFD}'))
                    .collect(),
            );
        }
    }
    if let Some(hex) = name.strip_prefix('u') {
        if (4..=6).contains(&hex.len()) {
            if let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                return Some(c.to_string());
            }
        }
    }
    let text = match name {
        "space" | "nbspace" | "nonbreakingspace" => " ",
        "exclam" => "!",
        "quotedbl" => "\"",
        "numbersign" => "#",
        "dollar" => "$",
        "percent" => "%",
        "ampersand" => "&",
        "quotesingle" => "'",
        "parenleft" => "(",
        "parenright" => ")",
        "asterisk" => "*",
        "plus" => "+",
        "comma" => ",",
        "hyphen" | "minus" | "sfthyphen" => "-",
        "period" => ".",
        "slash" => "/",
        "zero" => "0",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        "colon" => ":",
        "semicolon" => ";",
        "less" => "<",
        "equal" => "=",
        "greater" => ">",
        "question" => "?",
        "at" => "@",
        "bracketleft" => "[",
        "backslash" => "\\",
        "bracketright" => "]",
        "asciicircum" => "^",
        "underscore" => "_",
        "grave" => "`",
        "braceleft" => "{",
        "bar" => "|",
        "braceright" => "}",
        "asciitilde" => "~",
        "quoteleft" => "‘",
        "quoteright" => "’",
        "quotedblleft" => "“",
        "quotedblright" => "”",
        "quotesinglbase" => "‚",
        "quotedblbase" => "„",
        "guillemotleft" => "«",
        "guillemotright" => "»",
        "endash" => "–",
        "emdash" => "—",
        "bullet" => "•",
        "ellipsis" => "…",
        "Euro" => "€",
        "degree" => "°",
        "copyright" => "©",
        "registered" => "®",
        "trademark" => "™",
        "section" => "§",
        "paragraph" => "¶",
        "ordfeminine" => "ª",
        "ordmasculine" => "º",
        "questiondown" => "¿",
        "exclamdown" => "¡",
        "germandbls" => "ß",
        "ae" => "æ",
        "AE" => "Æ",
        "oe" => "œ",
        "OE" => "Œ",
        "oslash" => "ø",
        "Oslash" => "Ø",
        "fi" => "fi",
        "fl" => "fl",
        "ff" => "ff",
        "ffi" => "ffi",
        "ffl" => "ffl",
        "ccedilla" => "ç",
        "Ccedilla" => "Ç",
        "ntilde" => "ñ",
        "Ntilde" => "Ñ",
        "multiply" => "×",
        "divide" => "÷",
        "plusminus" => "±",
        "cent" => "¢",
        "sterling" => "£",
        "yen" => "¥",
        "periodcentered" | "middot" => "·",
        _ => return accented(name),
    };
    Some(text.to_string())
}

/// Letras con tilde, acento grave, circunflejo, dieresis, virgulilla o
/// anillo: `aacute`, `Egrave`, `udieresis`, `atilde`, `aring`...
fn accented(name: &str) -> Option<String> {
    const TABLE: [(&str, &str); 7] = [
        ("acute", "áéíóúýÁÉÍÓÚÝ"),
        ("grave", "àèìòùỳÀÈÌÒÙỲ"),
        ("circumflex", "âêîôûŷÂÊÎÔÛŶ"),
        ("dieresis", "äëïöüÿÄËÏÖÜŸ"),
        ("tilde", "ã\u{1EBD}ĩõũỹÃ\u{1EBC}ĨÕŨỸ"),
        ("ring", "å\0\0\0ů\0Å\0\0\0Ů\0"),
        ("caron", "ǎěǐǒǔ\0ǍĚǏǑǓ\0"),
    ];
    const BASES: &str = "aeiouyAEIOUY";
    let mut chars = name.chars();
    let base = chars.next()?;
    let suffix = chars.as_str();
    let index = BASES.find(base)?;
    let (_, letters) = TABLE.iter().find(|(s, _)| *s == suffix)?;
    let letter = letters.chars().nth(index)?;
    (letter != '\0').then(|| letter.to_string())
}

impl Pdf<'_> {
    fn build_font(&self, font: &Dict) -> Font {
        let composite = self.name(font.get("Subtype")) == Some("Type0");
        let cmap = self
            .stream_data(font.get("ToUnicode"))
            .map(|data| parse_cmap(&data));

        let encoding = font.get("Encoding").map(|e| self.resolve(e));
        let (base_name, differences) = match encoding {
            Some(Obj::Name(name)) => (Some(name.as_str()), None),
            Some(Obj::Dict(dict)) => (
                self.name(dict.get("BaseEncoding")),
                match dict.get("Differences").map(|d| self.resolve(d)) {
                    Some(Obj::Array(items)) => Some(items),
                    _ => None,
                },
            ),
            _ => (None, None),
        };
        let mut simple = base_encoding(base_name);
        if let Some(items) = differences {
            let mut code = 0usize;
            for item in items {
                match item {
                    Obj::Num(n) => code = (*n).max(0.0) as usize,
                    Obj::Name(name) => {
                        if code < 256 {
                            if let Some(text) = glyph_text(name) {
                                simple[code] = text;
                            }
                        }
                        code += 1;
                    }
                    _ => {}
                }
            }
        }

        let mut first_char = 0;
        let mut widths = Vec::new();
        let mut cid_widths = HashMap::new();
        let default_width;
        if composite {
            // Los anchos estan en la fuente descendiente: /DW (por defecto)
            // y /W, con entradas `c [w1 w2 ...]` o `c_inicial c_final w`.
            let descendant = match font.get("DescendantFonts").map(|d| self.resolve(d)) {
                Some(Obj::Array(items)) => items.first().and_then(|d| self.dict(Some(d))),
                _ => None,
            };
            default_width = descendant
                .and_then(|d| self.number(d.get("DW")))
                .unwrap_or(1000.0);
            if let Some(Obj::Array(list)) =
                descendant.and_then(|d| d.get("W")).map(|w| self.resolve(w))
            {
                let mut i = 0;
                while i < list.len() && cid_widths.len() < MAX_CMAP_ENTRIES {
                    let Some(first) = self.resolve(&list[i]).as_num() else {
                        break;
                    };
                    let first = first.max(0.0) as u32;
                    match list.get(i + 1).map(|x| self.resolve(x)) {
                        Some(Obj::Array(values)) => {
                            for (offset, value) in values.iter().enumerate() {
                                if let Some(value) = self.resolve(value).as_num() {
                                    cid_widths.insert(first.saturating_add(offset as u32), value);
                                }
                            }
                            i += 2;
                        }
                        Some(Obj::Num(last)) => {
                            let last = last.max(0.0) as u32;
                            let value = list
                                .get(i + 2)
                                .and_then(|x| self.resolve(x).as_num())
                                .unwrap_or(default_width);
                            if last >= first && last - first <= 0xFFFF {
                                for code in first..=last {
                                    cid_widths.insert(code, value);
                                }
                            }
                            i += 3;
                        }
                        _ => break,
                    }
                }
            }
        } else {
            first_char = self.number(font.get("FirstChar")).unwrap_or(0.0).max(0.0) as u32;
            if let Some(Obj::Array(items)) = font.get("Widths").map(|w| self.resolve(w)) {
                widths = items
                    .iter()
                    .take(65536)
                    .map(|w| self.resolve(w).as_num().unwrap_or(0.0))
                    .collect();
            }
            // Sin datos, se supone medio cuadratin (ancho tipico de letra).
            default_width = self
                .dict(font.get("FontDescriptor"))
                .and_then(|d| self.number(d.get("MissingWidth")))
                .filter(|w| *w > 0.0)
                .unwrap_or(500.0);
        }

        Font {
            composite,
            cmap,
            simple,
            first_char,
            widths,
            cid_widths,
            default_width,
        }
    }
}

// ---------------------------------------------------------------------------
// Interpretacion del contenido de una pagina
// ---------------------------------------------------------------------------
//
// Para decidir donde van los saltos de linea y los espacios se sigue la
// posicion real de cada trozo de texto, como hace un visor de PDF:
// - La matriz de texto (`Tm`, `Td`...) y la de transformacion (`cm`, `q`, `Q`)
//   dan la posicion en la pagina donde empieza cada trozo.
// - El ancho de cada glifo (de la fuente) y los espaciados (`Tc`, `Tw`, `Tz`)
//   dan donde termina.
// Si un trozo empieza a otra altura (mas de media letra) es otra linea; si
// empieza a la misma altura pero dejando un hueco, se separa con un espacio.

/// Matriz afin de PDF `[a b c d e f]`.
type Matrix = [f64; 6];

const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Producto de matrices `m x n` (primero se aplica `m`).
fn multiply(m: &Matrix, n: &Matrix) -> Matrix {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn matrix_from(operands: &[Obj]) -> Option<Matrix> {
    let values: Vec<f64> = operands.iter().filter_map(Obj::as_num).collect();
    let values: [f64; 6] = values.get(..6)?.try_into().ok()?;
    values.iter().all(|v| v.is_finite()).then_some(values)
}

/// Estado de texto y graficos que afecta a la posicion del texto.
#[derive(Clone)]
struct TextState {
    tm: Matrix,
    tlm: Matrix,
    ctm: Matrix,
    font: Option<Rc<Font>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
}

impl TextState {
    fn new(ctm: Matrix) -> Self {
        TextState {
            tm: IDENTITY,
            tlm: IDENTITY,
            ctm,
            font: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
        }
    }

    /// Posicion en la pagina del punto actual y altura de letra efectiva.
    fn position(&self) -> (f64, f64, f64) {
        let m = multiply(&self.tm, &self.ctm);
        let height = (self.size * (m[2] * m[2] + m[3] * m[3]).sqrt()).abs();
        (
            m[4],
            m[5],
            if height.is_finite() {
                height.max(0.1)
            } else {
                1.0
            },
        )
    }

    fn move_text(&mut self, tx: f64, ty: f64) {
        self.tlm = multiply(&[1.0, 0.0, 0.0, 1.0, tx, ty], &self.tlm);
        self.tm = self.tlm;
    }

    /// Avanza horizontalmente `tx` unidades de texto.
    fn advance(&mut self, tx: f64) {
        self.tm[4] += tx * self.tm[0];
        self.tm[5] += tx * self.tm[1];
    }
}

struct TextOut {
    text: String,
    /// Donde termino el ultimo trozo escrito: (x, y, altura de letra).
    last: Option<(f64, f64, f64)>,
}

impl TextOut {
    fn newline(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    fn space(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with([' ', '\n']) {
            self.text.push(' ');
        }
    }

    /// Antes de escribir un trozo en (x, y): decide si va en una linea nueva
    /// o separado por un espacio del anterior.
    fn place(&mut self, x: f64, y: f64, height: f64) {
        if let Some((last_x, last_y, last_height)) = self.last {
            let letter = height.max(last_height);
            if (y - last_y).abs() > letter * 0.5 {
                self.newline();
            } else if (x - last_x) > letter * 0.15 || (x - last_x) < -letter {
                self.space();
            }
        }
    }
}

type FontCache = HashMap<u32, Rc<Font>>;

impl Pdf<'_> {
    fn font_for(
        &self,
        resources: Option<&Dict>,
        name: &str,
        cache: &mut FontCache,
    ) -> Option<Rc<Font>> {
        let fonts = self.dict(resources?.get("Font"))?;
        let entry = fonts.get(name)?;
        if let Obj::Ref(number) = entry {
            if let Some(font) = cache.get(number) {
                return Some(Rc::clone(font));
            }
            let font = Rc::new(self.build_font(self.dict(Some(entry))?));
            cache.insert(*number, Rc::clone(&font));
            return Some(font);
        }
        Some(Rc::new(self.build_font(self.dict(Some(entry))?)))
    }

    /// Escribe un texto (operando de `Tj`, `TJ`, `'` o `"`) y avanza la
    /// posicion segun el ancho de sus glifos.
    fn show(state: &mut TextState, bytes: &[u8], out: &mut TextOut) {
        let (x, y, height) = state.position();
        out.place(x, y, height);
        let font = state.font.clone();
        let mut glyph = |text: &str, width: f64, is_space: bool| {
            out.text.push_str(text);
            let spacing = state.char_spacing + if is_space { state.word_spacing } else { 0.0 };
            state.advance((width / 1000.0 * state.size + spacing) * state.scale);
        };
        match font {
            Some(font) => font.glyphs(bytes, &mut glyph),
            None => {
                for &byte in bytes {
                    glyph(&crate_cp1252(byte), 500.0, byte == 32);
                }
            }
        }
        let (x, y, _) = state.position();
        out.last = Some((x, y, height));
    }

    /// Interpreta un flujo de contenido y agrega su texto a `out`.
    fn run_content(
        &self,
        content: &[u8],
        resources: Option<&Dict>,
        out: &mut TextOut,
        cache: &mut FontCache,
        depth: usize,
        ctm: Matrix,
    ) {
        let mut lexer = Lexer::new(content, 0);
        let mut operands: Vec<Obj> = Vec::new();
        let mut state = TextState::new(ctm);
        let mut saved: Vec<TextState> = Vec::new();

        while let Some(token) = lexer.token() {
            let Tok::Word(op) = token else {
                if let Some(value) = lexer.value_from(token, 0) {
                    operands.push(value);
                }
                if operands.len() > 64 {
                    operands.clear(); // contenido raro: evita crecer sin limite
                }
                continue;
            };
            let num = |index: usize| {
                operands
                    .get(index)
                    .and_then(Obj::as_num)
                    .filter(|v| v.is_finite())
                    .unwrap_or(0.0)
            };
            match op {
                b"q" if saved.len() < 256 => saved.push(state.clone()),
                b"Q" => {
                    if let Some(previous) = saved.pop() {
                        state = previous;
                    }
                }
                b"cm" => {
                    if let Some(m) = matrix_from(&operands) {
                        state.ctm = multiply(&m, &state.ctm);
                    }
                }
                b"BT" => {
                    state.tm = IDENTITY;
                    state.tlm = IDENTITY;
                }
                b"Tf" => {
                    if let Some(Obj::Name(name)) = operands.first() {
                        state.font = self.font_for(resources, name, cache);
                    }
                    state.size = num(1);
                }
                b"Tc" => state.char_spacing = num(0),
                b"Tw" => state.word_spacing = num(0),
                b"Tz" => state.scale = num(0) / 100.0,
                b"TL" => state.leading = num(0),
                b"Td" => state.move_text(num(0), num(1)),
                b"TD" => {
                    state.leading = -num(1);
                    state.move_text(num(0), num(1));
                }
                b"Tm" => {
                    if let Some(m) = matrix_from(&operands) {
                        state.tm = m;
                        state.tlm = m;
                    }
                }
                b"T*" => state.move_text(0.0, -state.leading),
                b"Tj" => {
                    if let Some(Obj::Str(bytes)) = operands.last() {
                        Self::show(&mut state, bytes, out);
                    }
                }
                b"'" | b"\"" => {
                    if op == b"\"" {
                        state.word_spacing = num(0);
                        state.char_spacing = num(1);
                    }
                    state.move_text(0.0, -state.leading);
                    if let Some(Obj::Str(bytes)) = operands.last() {
                        Self::show(&mut state, bytes, out);
                    }
                }
                b"TJ" => {
                    if let Some(Obj::Array(items)) = operands.last() {
                        for item in items {
                            match item {
                                Obj::Str(bytes) => Self::show(&mut state, bytes, out),
                                // Ajuste de posicion en milesimas de letra
                                // (negativo = hacia la derecha).
                                Obj::Num(n) if n.is_finite() => {
                                    state.advance(-n / 1000.0 * state.size * state.scale)
                                }
                                _ => {}
                            }
                        }
                    }
                }
                b"Do" if depth < MAX_FORM_DEPTH => {
                    // Formularios (XObject /Form): contenido reutilizable que
                    // tambien puede tener texto (encabezados, sellos...).
                    if let Some(Obj::Name(name)) = operands.first() {
                        let xobject = resources
                            .and_then(|r| self.dict(r.get("XObject")))
                            .and_then(|x| x.get(name));
                        if let Some(Obj::Stream(dict, start, end)) =
                            xobject.map(|x| self.resolve(x))
                        {
                            if self.name(dict.get("Subtype")) == Some("Form") {
                                if let Some(data) = self.decode_stream(dict, *start, *end) {
                                    let inner = self.dict(dict.get("Resources")).or(resources);
                                    let matrix = match dict.get("Matrix").map(|m| self.resolve(m)) {
                                        Some(Obj::Array(items)) => {
                                            matrix_from(items).unwrap_or(IDENTITY)
                                        }
                                        _ => IDENTITY,
                                    };
                                    let form_ctm = multiply(&matrix, &state.ctm);
                                    self.run_content(&data, inner, out, cache, depth + 1, form_ctm);
                                }
                            }
                        }
                    }
                }
                b"BI" => {
                    // Imagen incrustada: sus datos binarios van entre ID y EI.
                    if let Some(id) = find(content, b"ID", lexer.pos) {
                        let mut pos = id + 2;
                        lexer.pos = content.len();
                        while let Some(ei) = find(content, b"EI", pos) {
                            let before = ei == 0 || is_space(content[ei - 1]);
                            let after = content.get(ei + 2).is_none_or(|&b| is_space(b));
                            if before && after {
                                lexer.pos = ei + 2;
                                break;
                            }
                            pos = ei + 2;
                        }
                    }
                }
                _ => {}
            }
            operands.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Punto de entrada
// ---------------------------------------------------------------------------

pub fn extract(path: &Path) -> io::Result<Vec<Unit>> {
    if fs::metadata(path)?.len() > MAX_PDF_BYTES {
        return Err(invalid("PDF demasiado grande (mas de 512 MiB)"));
    }
    let data = fs::read(path)?;
    if find(&data[..data.len().min(1024)], b"%PDF", 0).is_none() {
        return Err(invalid("no es un archivo PDF valido"));
    }

    let pdf = Pdf::parse(&data);
    if pdf.encrypted {
        return Err(invalid(
            "PDF protegido (cifrado): no se puede leer su texto",
        ));
    }

    let mut units = Vec::new();
    let mut cache = FontCache::new();
    for (index, (page, resources)) in pdf.pages().into_iter().enumerate() {
        let content = pdf.page_content(page);
        let mut out = TextOut {
            text: String::new(),
            last: None,
        };
        pdf.run_content(&content, resources, &mut out, &mut cache, 0, IDENTITY);
        for line in out.text.lines() {
            let line = line.trim();
            if !line.is_empty() {
                units.push(Unit {
                    location: format!("pagina {}", index + 1),
                    text: line.to_string(),
                });
            }
        }
    }
    Ok(units)
}
