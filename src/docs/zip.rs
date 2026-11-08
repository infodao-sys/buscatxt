//! Lector minimo de archivos ZIP: solo lo necesario para leer las partes XML
//! de los documentos de Office (DOCX, XLSX, PPTX) y de LibreOffice (ODT,
//! ODS, ODP), que son archivos ZIP por dentro.
//!
//! Solo lee las entradas que se piden (no descomprime imagenes ni otros
//! archivos que no hacen falta) y valida todos los tamanos y posiciones antes
//! de usarlos. No admite ZIP cifrados ni ZIP64 (archivos de mas de 4 GiB).

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use super::inflate::{inflate, InflateError};
use super::invalid;

/// Tamano maximo del directorio central (indice del ZIP).
const MAX_CENTRAL_DIR: u64 = 64 * 1024 * 1024;

/// Una entrada (archivo) dentro del ZIP.
pub struct Entry {
    pub name: String,
    method: u16,
    flags: u16,
    compressed_size: u64,
    size: u64,
    offset: u64,
}

pub struct Archive {
    file: File,
    file_len: u64,
    pub entries: Vec<Entry>,
}

/// Como `read_exact`, pero si el archivo termina antes de tiempo devuelve un
/// mensaje claro en vez de "failed to fill whole buffer".
fn read_exact(file: &mut File, buf: &mut [u8]) -> io::Result<()> {
    file.read_exact(buf).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid("archivo ZIP danado o truncado")
        } else {
            error
        }
    })
}

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

impl Archive {
    /// Abre el ZIP y lee su indice (el "directorio central", al final del
    /// archivo).
    pub fn open(path: &Path) -> io::Result<Archive> {
        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();
        let not_zip = || {
            invalid(
                "no es un archivo ZIP valido (puede estar danado, protegido con \
                 contrasena o ser un formato antiguo como .doc/.xls/.ppt)",
            )
        };

        // El registro final ("End of Central Directory") mide 22 bytes mas
        // un comentario de hasta 65535 bytes.
        let tail_len = file_len.min(22 + 65535);
        file.seek(SeekFrom::Start(file_len - tail_len))?;
        let mut tail = vec![0u8; tail_len as usize];
        read_exact(&mut file, &mut tail)?;
        let eocd = tail
            .windows(4)
            .rposition(|w| w == [0x50, 0x4B, 0x05, 0x06])
            .ok_or_else(not_zip)?;

        let count = u16_at(&tail, eocd + 10).ok_or_else(not_zip)?;
        let dir_size = u32_at(&tail, eocd + 12).ok_or_else(not_zip)?;
        let dir_offset = u32_at(&tail, eocd + 16).ok_or_else(not_zip)?;
        if count == 0xFFFF || dir_size == 0xFFFF_FFFF || dir_offset == 0xFFFF_FFFF {
            return Err(invalid("archivo ZIP64 (mayor de 4 GiB) no soportado"));
        }
        let (dir_size, dir_offset) = (u64::from(dir_size), u64::from(dir_offset));
        if dir_size > MAX_CENTRAL_DIR || dir_offset + dir_size > file_len {
            return Err(not_zip());
        }

        let mut dir = vec![0u8; dir_size as usize];
        file.seek(SeekFrom::Start(dir_offset))?;
        read_exact(&mut file, &mut dir)?;

        let mut entries = Vec::with_capacity(usize::from(count));
        let mut pos = 0;
        while entries.len() < usize::from(count) {
            if u32_at(&dir, pos) != Some(0x0201_4B50) {
                return Err(not_zip());
            }
            let field = |offset: usize| u16_at(&dir, pos + offset).ok_or_else(not_zip);
            let field32 = |offset: usize| u32_at(&dir, pos + offset).ok_or_else(not_zip);
            let flags = field(8)?;
            let method = field(10)?;
            let compressed_size = u64::from(field32(20)?);
            let size = u64::from(field32(24)?);
            let name_len = usize::from(field(28)?);
            let extra_len = usize::from(field(30)?);
            let comment_len = usize::from(field(32)?);
            let offset = u64::from(field32(42)?);
            let name = dir.get(pos + 46..pos + 46 + name_len).ok_or_else(not_zip)?;
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                method,
                flags,
                compressed_size,
                size,
                offset,
            });
            pos += 46 + name_len + extra_len + comment_len;
        }

        Ok(Archive {
            file,
            file_len,
            entries,
        })
    }

    /// Lee y descomprime una entrada. Devuelve `None` si no existe.
    /// `limit` es el tamano maximo descomprimido aceptado.
    pub fn read(&mut self, name: &str, limit: usize) -> io::Result<Option<Vec<u8>>> {
        let Some(entry) = self.entries.iter().find(|entry| entry.name == name) else {
            return Ok(None);
        };
        if entry.flags & 1 != 0 {
            return Err(invalid("documento protegido con contrasena"));
        }
        if entry.size > limit as u64 {
            return Err(invalid(&format!("la parte {name} es demasiado grande")));
        }
        let (method, compressed_size, offset) = (entry.method, entry.compressed_size, entry.offset);

        // Cabecera local: 30 bytes fijos + nombre + campo extra.
        let mut header = [0u8; 30];
        self.file.seek(SeekFrom::Start(offset))?;
        read_exact(&mut self.file, &mut header)?;
        if u32_at(&header, 0) != Some(0x0403_4B50) {
            return Err(invalid("archivo ZIP danado"));
        }
        let name_len = u64::from(u16_at(&header, 26).unwrap_or(0));
        let extra_len = u64::from(u16_at(&header, 28).unwrap_or(0));
        let start = offset + 30 + name_len + extra_len;
        if start + compressed_size > self.file_len {
            return Err(invalid("archivo ZIP danado o truncado"));
        }

        let mut compressed = vec![0u8; compressed_size as usize];
        self.file.seek(SeekFrom::Start(start))?;
        read_exact(&mut self.file, &mut compressed)?;

        match method {
            0 => Ok(Some(compressed)),
            8 => match inflate(&compressed, limit) {
                (data, None) => Ok(Some(data)),
                (_, Some(InflateError::TooLarge)) => {
                    Err(invalid(&format!("la parte {name} es demasiado grande")))
                }
                (_, Some(InflateError::Corrupt)) => Err(invalid(&format!(
                    "la parte {name} esta danada (o el documento esta protegido con contrasena)"
                ))),
            },
            other => Err(invalid(&format!(
                "metodo de compresion ZIP {other} no soportado"
            ))),
        }
    }
}
