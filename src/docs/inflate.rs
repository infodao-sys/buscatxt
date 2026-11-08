//! Descompresor DEFLATE (RFC 1951) y envoltorio zlib (RFC 1950).
//!
//! Lo usan los documentos ZIP (DOCX, XLSX, PPTX, ODT, ODS, ODP) y los flujos
//! comprimidos de los PDF (`/FlateDecode`). Esta escrito siguiendo la
//! implementacion de referencia "puff" de zlib: prioriza la claridad y la
//! seguridad sobre la velocidad.
//!
//! Seguridad:
//! - Solo Rust seguro: todos los accesos a memoria estan comprobados.
//! - `limit` acota el tamano descomprimido. Un archivo malicioso pequeno que
//!   se expande a gigabytes ("bomba de descompresion") se corta al llegar al
//!   limite en vez de agotar la memoria.
//! - Los datos corruptos producen un error, nunca un bucle infinito: cada
//!   iteracion consume bits de la entrada o termina.

/// Motivo por el que se detuvo la descompresion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// Datos invalidos o truncados.
    Corrupt,
    /// El resultado supera el limite permitido.
    TooLarge,
}

/// Lector de bits: DEFLATE guarda los datos empezando por el bit menos
/// significativo de cada byte.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u32,
    count: u32,
}

impl Bits<'_> {
    /// Lee `need` bits (como maximo 16).
    fn bits(&mut self, need: u32) -> Result<u32, InflateError> {
        let mut value = self.buf;
        while self.count < need {
            let byte = *self.data.get(self.pos).ok_or(InflateError::Corrupt)?;
            self.pos += 1;
            value |= u32::from(byte) << self.count;
            self.count += 8;
        }
        self.buf = value >> need;
        self.count -= need;
        Ok(value & ((1u32 << need) - 1))
    }
}

/// Tabla de Huffman canonica: cuantos codigos hay de cada largo y los
/// simbolos ordenados por codigo.
struct Huffman {
    count: [u16; 16],
    symbol: Vec<u16>,
}

impl Huffman {
    /// Construye la tabla a partir del largo de codigo de cada simbolo.
    /// Rechaza tablas "sobresuscritas" (mas codigos de los posibles).
    fn new(lengths: &[u8]) -> Result<Self, InflateError> {
        let mut count = [0u16; 16];
        for &len in lengths {
            count[usize::from(len)] += 1;
        }
        let mut left: i32 = 1;
        for &c in &count[1..] {
            left <<= 1;
            left -= i32::from(c);
            if left < 0 {
                return Err(InflateError::Corrupt);
            }
        }
        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0u16; lengths.len()];
        for (sym, &len) in lengths.iter().enumerate() {
            if len != 0 {
                let slot = &mut offs[usize::from(len)];
                symbol[usize::from(*slot)] = sym as u16;
                *slot += 1;
            }
        }
        Ok(Huffman { count, symbol })
    }

    /// Decodifica un simbolo leyendo bit a bit.
    fn decode(&self, bits: &mut Bits) -> Result<u16, InflateError> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= bits.bits(1)? as i32;
            let count = i32::from(self.count[len]);
            if code - count < first {
                return self
                    .symbol
                    .get((index + (code - first)) as usize)
                    .copied()
                    .ok_or(InflateError::Corrupt);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::Corrupt)
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Orden en que se guardan los largos de la tabla de largos de codigo.
const CODE_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Decodifica los simbolos de un bloque comprimido hasta el fin de bloque.
fn codes(
    bits: &mut Bits,
    out: &mut Vec<u8>,
    limit: usize,
    lencode: &Huffman,
    distcode: &Huffman,
) -> Result<(), InflateError> {
    loop {
        let symbol = lencode.decode(bits)?;
        match symbol {
            0..=255 => {
                if out.len() >= limit {
                    return Err(InflateError::TooLarge);
                }
                out.push(symbol as u8);
            }
            256 => return Ok(()),
            _ => {
                // Copia de una secuencia anterior: (largo, distancia).
                let index = usize::from(symbol - 257);
                if index >= LEN_BASE.len() {
                    return Err(InflateError::Corrupt);
                }
                let len =
                    usize::from(LEN_BASE[index]) + bits.bits(u32::from(LEN_EXTRA[index]))? as usize;
                let dsym = usize::from(distcode.decode(bits)?);
                if dsym >= DIST_BASE.len() {
                    return Err(InflateError::Corrupt);
                }
                let dist =
                    usize::from(DIST_BASE[dsym]) + bits.bits(u32::from(DIST_EXTRA[dsym]))? as usize;
                if dist > out.len() {
                    return Err(InflateError::Corrupt);
                }
                if out.len() + len > limit {
                    return Err(InflateError::TooLarge);
                }
                // Byte a byte, porque origen y destino pueden solaparse.
                let start = out.len() - dist;
                for i in 0..len {
                    let byte = out[start + i];
                    out.push(byte);
                }
            }
        }
    }
}

/// Bloque sin comprimir.
fn stored(bits: &mut Bits, out: &mut Vec<u8>, limit: usize) -> Result<(), InflateError> {
    // Se descartan los bits pendientes: el bloque empieza en un byte entero.
    bits.buf = 0;
    bits.count = 0;
    let header = bits
        .data
        .get(bits.pos..bits.pos + 4)
        .ok_or(InflateError::Corrupt)?;
    let len = usize::from(u16::from_le_bytes([header[0], header[1]]));
    let nlen = u16::from_le_bytes([header[2], header[3]]);
    if len as u16 != !nlen {
        return Err(InflateError::Corrupt);
    }
    bits.pos += 4;
    let chunk = bits
        .data
        .get(bits.pos..bits.pos + len)
        .ok_or(InflateError::Corrupt)?;
    if out.len() + len > limit {
        return Err(InflateError::TooLarge);
    }
    out.extend_from_slice(chunk);
    bits.pos += len;
    Ok(())
}

/// Bloque con las tablas de Huffman fijas del estandar.
fn fixed(bits: &mut Bits, out: &mut Vec<u8>, limit: usize) -> Result<(), InflateError> {
    let mut lengths = [0u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    let lencode = Huffman::new(&lengths)?;
    let distcode = Huffman::new(&[5u8; 30])?;
    codes(bits, out, limit, &lencode, &distcode)
}

/// Bloque con tablas de Huffman propias, guardadas al inicio del bloque.
fn dynamic(bits: &mut Bits, out: &mut Vec<u8>, limit: usize) -> Result<(), InflateError> {
    let nlen = bits.bits(5)? as usize + 257;
    let ndist = bits.bits(5)? as usize + 1;
    let ncode = bits.bits(4)? as usize + 4;
    if nlen > 286 || ndist > 30 {
        return Err(InflateError::Corrupt);
    }

    let mut code_lengths = [0u8; 19];
    for &index in CODE_ORDER.iter().take(ncode) {
        code_lengths[index] = bits.bits(3)? as u8;
    }
    let lencode = Huffman::new(&code_lengths)?;

    let mut lengths = [0u8; 316];
    let mut index = 0;
    while index < nlen + ndist {
        let symbol = lencode.decode(bits)?;
        if symbol < 16 {
            lengths[index] = symbol as u8;
            index += 1;
            continue;
        }
        let (value, repeat) = match symbol {
            16 => {
                if index == 0 {
                    return Err(InflateError::Corrupt);
                }
                (lengths[index - 1], 3 + bits.bits(2)? as usize)
            }
            17 => (0, 3 + bits.bits(3)? as usize),
            _ => (0, 11 + bits.bits(7)? as usize),
        };
        if index + repeat > nlen + ndist {
            return Err(InflateError::Corrupt);
        }
        lengths[index..index + repeat].fill(value);
        index += repeat;
    }
    if lengths[256] == 0 {
        return Err(InflateError::Corrupt);
    }

    let lencode = Huffman::new(&lengths[..nlen])?;
    let distcode = Huffman::new(&lengths[nlen..nlen + ndist])?;
    codes(bits, out, limit, &lencode, &distcode)
}

/// Descomprime datos DEFLATE "crudos". Devuelve lo descomprimido y, si hubo
/// un problema, el motivo. En caso de error se devuelve igualmente lo que se
/// pudo recuperar (util con PDF truncados).
pub fn inflate(data: &[u8], limit: usize) -> (Vec<u8>, Option<InflateError>) {
    let mut out = Vec::with_capacity(data.len().saturating_mul(4).min(limit).min(1 << 20));
    let mut bits = Bits {
        data,
        pos: 0,
        buf: 0,
        count: 0,
    };
    loop {
        let result = (|| {
            let last = bits.bits(1)?;
            match bits.bits(2)? {
                0 => stored(&mut bits, &mut out, limit)?,
                1 => fixed(&mut bits, &mut out, limit)?,
                2 => dynamic(&mut bits, &mut out, limit)?,
                _ => return Err(InflateError::Corrupt),
            }
            Ok(last == 1)
        })();
        match result {
            Ok(true) => return (out, None),
            Ok(false) => continue,
            Err(error) => return (out, Some(error)),
        }
    }
}

/// Descomprime datos con cabecera zlib (la usan los PDF). Si la cabecera no
/// es valida se intenta como DEFLATE crudo, porque hay PDF que la omiten.
pub fn zlib(data: &[u8], limit: usize) -> (Vec<u8>, Option<InflateError>) {
    if data.len() >= 2 {
        let cmf = data[0];
        let flg = data[1];
        let valid = cmf & 0x0F == 8 && (u16::from(cmf) << 8 | u16::from(flg)) % 31 == 0;
        if valid {
            let skip = if flg & 0x20 != 0 { 6 } else { 2 };
            return inflate(data.get(skip..).unwrap_or(&[]), limit);
        }
    }
    inflate(data, limit)
}
