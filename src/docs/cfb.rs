//! Lector de "Compound File Binary" (CFB, tambien llamado OLE2): el
//! contenedor de los formatos antiguos de Office (`.doc`, `.xls`, `.ppt`).
//!
//! Un archivo CFB es como un pequeno sistema de archivos FAT dentro de un
//! archivo: el contenido se divide en sectores (512 o 4096 bytes), una tabla
//! FAT indica que sector sigue a cada uno, y un directorio da el nombre, el
//! primer sector y el tamano de cada "flujo" (archivo interno). Los flujos
//! pequenos (menos de 4096 bytes) se guardan en un "mini flujo" con sectores
//! de 64 bytes y su propia tabla (MiniFAT).
//!
//! Seguridad: todas las posiciones se comprueban antes de usarlas, las
//! cadenas de sectores no pueden tener ciclos (se cortan si superan la
//! cantidad de sectores existentes) y el tamano de los flujos esta acotado.

use std::fs;
use std::io;
use std::path::Path;

use super::{invalid, MAX_DOC_BYTES, MAX_PART_BYTES};

/// Tamano maximo de un archivo CFB que se carga en memoria.
const MAX_CFB_BYTES: u64 = MAX_DOC_BYTES as u64;

const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
const NO_STREAM: u32 = 0xFFFF_FFFF;

/// Entrada del directorio.
struct Entry {
    name: String,
    kind: u8,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

pub struct Compound {
    data: Vec<u8>,
    sector_size: usize,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<Entry>,
}

fn u16_at(data: &[u8], pos: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(pos..pos + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

/// Indica si el archivo empieza con la firma de CFB.
pub fn is_compound(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 8];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok_and(|()| head == SIGNATURE)
}

impl Compound {
    pub fn open(path: &Path) -> io::Result<Compound> {
        if fs::metadata(path)?.len() > MAX_CFB_BYTES {
            return Err(invalid("documento demasiado grande"));
        }
        let data = fs::read(path)?;
        let bad = || invalid("documento de Office antiguo danado (contenedor OLE invalido)");
        if data.get(..8) != Some(&SIGNATURE) {
            return Err(invalid(
                "no es un documento de Office 97-2003 valido (puede estar danado o ser otro formato)",
            ));
        }

        let sector_shift = u16_at(&data, 0x1E).ok_or_else(bad)?;
        if sector_shift != 9 && sector_shift != 12 {
            return Err(bad());
        }
        let sector_size = 1usize << sector_shift;
        let fat_sectors = u32_at(&data, 0x2C).ok_or_else(bad)? as usize;
        let dir_start = u32_at(&data, 0x30).ok_or_else(bad)?;
        let mini_cutoff = u32_at(&data, 0x38).ok_or_else(bad)?;
        let mini_fat_start = u32_at(&data, 0x3C).ok_or_else(bad)?;
        let mut difat_next = u32_at(&data, 0x44).ok_or_else(bad)?;
        let total_sectors = data.len() / sector_size;
        if mini_cutoff != 4096 || fat_sectors > total_sectors {
            return Err(bad());
        }

        let mut compound = Compound {
            data,
            sector_size,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            entries: Vec::new(),
        };

        // DIFAT: lista de los sectores que forman la FAT. Las primeras 109
        // entradas estan en la cabecera; el resto, en una cadena de sectores.
        let mut fat_sector_ids: Vec<u32> = (0..109)
            .filter_map(|i| u32_at(&compound.data, 0x4C + i * 4))
            .filter(|&id| id < END_OF_CHAIN)
            .collect();
        let per_sector = sector_size / 4;
        let mut guard = 0;
        while difat_next < END_OF_CHAIN && guard <= total_sectors {
            guard += 1;
            let sector = compound.sector(difat_next).ok_or_else(bad)?;
            for i in 0..per_sector - 1 {
                let id = u32_at(sector, i * 4).ok_or_else(bad)?;
                if id < END_OF_CHAIN {
                    fat_sector_ids.push(id);
                }
            }
            difat_next = u32_at(sector, (per_sector - 1) * 4).ok_or_else(bad)?;
        }
        fat_sector_ids.truncate(fat_sectors);

        let mut fat = Vec::with_capacity(fat_sector_ids.len() * per_sector);
        for id in fat_sector_ids {
            let sector = compound.sector(id).ok_or_else(bad)?;
            fat.extend((0..per_sector).filter_map(|i| u32_at(sector, i * 4)));
        }
        compound.fat = fat;

        // Directorio: entradas de 128 bytes.
        let dir = compound
            .chain(dir_start, None, MAX_PART_BYTES)
            .ok_or_else(bad)?;
        for raw in dir.as_chunks::<128>().0 {
            let name_len = usize::from(u16_at(raw, 0x40).unwrap_or(0)).min(64);
            let units: Vec<u16> = raw[..name_len]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .take_while(|&u| u != 0)
                .collect();
            compound.entries.push(Entry {
                name: String::from_utf16_lossy(&units),
                kind: raw[0x42],
                left: u32_at(raw, 0x44).unwrap_or(NO_STREAM),
                right: u32_at(raw, 0x48).unwrap_or(NO_STREAM),
                child: u32_at(raw, 0x4C).unwrap_or(NO_STREAM),
                start: u32_at(raw, 0x74).unwrap_or(END_OF_CHAIN),
                // Los archivos de version 3 solo usan los 32 bits bajos.
                size: if sector_size == 512 {
                    u64::from(u32_at(raw, 0x78).unwrap_or(0))
                } else {
                    u64::from(u32_at(raw, 0x78).unwrap_or(0))
                        | u64::from(u32_at(raw, 0x7C).unwrap_or(0)) << 32
                },
            });
        }
        let root = compound.entries.first().ok_or_else(bad)?;
        if root.kind != 5 {
            return Err(bad());
        }

        // Mini flujo (guardado en la cadena de la entrada raiz) y su tabla.
        let (root_start, root_size) = (root.start, root.size);
        compound.mini_stream = compound
            .chain(root_start, Some(root_size), MAX_PART_BYTES)
            .unwrap_or_default();
        let mini_fat_bytes = compound
            .chain(mini_fat_start, None, MAX_PART_BYTES)
            .unwrap_or_default();
        compound.mini_fat = mini_fat_bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from_le_bytes([p[0], p[1], p[2], p[3]]))
            .collect();
        Ok(compound)
    }

    fn sector(&self, id: u32) -> Option<&[u8]> {
        // El sector 0 empieza despues de la cabecera (que ocupa un sector).
        let start = (id as usize)
            .checked_add(1)?
            .checked_mul(self.sector_size)?;
        self.data.get(start..start + self.sector_size)
    }

    /// Concatena una cadena de sectores de la FAT. `size` recorta al tamano
    /// real del flujo.
    fn chain(&self, start: u32, size: Option<u64>, limit: usize) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut id = start;
        let mut steps = 0;
        while id < END_OF_CHAIN {
            steps += 1;
            if steps > self.fat.len() + 1 || out.len() > limit {
                return None; // ciclo o tamano excesivo
            }
            out.extend_from_slice(self.sector(id)?);
            id = *self.fat.get(id as usize)?;
        }
        if let Some(size) = size {
            out.truncate(size.min(usize::MAX as u64) as usize);
        }
        Some(out)
    }

    fn mini_chain(&self, start: u32, size: u64) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut id = start;
        let mut steps = 0;
        while id < END_OF_CHAIN && (out.len() as u64) < size {
            steps += 1;
            if steps > self.mini_fat.len() + 1 {
                return None;
            }
            let pos = (id as usize).checked_mul(64)?;
            out.extend_from_slice(self.mini_stream.get(pos..pos + 64)?);
            id = *self.mini_fat.get(id as usize)?;
        }
        out.truncate(size as usize);
        Some(out)
    }

    /// Indices de las entradas hijas directas de la raiz (recorriendo el
    /// arbol rojo-negro del directorio, sin recursion).
    fn root_children(&self) -> Vec<usize> {
        let mut result = Vec::new();
        let mut stack = vec![self.entries.first().map_or(NO_STREAM, |root| root.child)];
        let mut visited = vec![false; self.entries.len()];
        while let Some(id) = stack.pop() {
            let index = id as usize;
            if id >= NO_STREAM - 1 || index >= self.entries.len() || visited[index] {
                continue;
            }
            visited[index] = true;
            result.push(index);
            stack.push(self.entries[index].left);
            stack.push(self.entries[index].right);
        }
        result
    }

    /// Indica si existe un flujo con ese nombre en la raiz.
    pub fn has_stream(&self, name: &str) -> bool {
        self.root_children()
            .into_iter()
            .any(|i| self.entries[i].kind == 2 && self.entries[i].name.eq_ignore_ascii_case(name))
    }

    /// Contenido de un flujo de la raiz. `None` si no existe.
    pub fn stream(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let Some(index) = self.root_children().into_iter().find(|&i| {
            self.entries[i].kind == 2 && self.entries[i].name.eq_ignore_ascii_case(name)
        }) else {
            return Ok(None);
        };
        let entry = &self.entries[index];
        if entry.size > MAX_PART_BYTES as u64 {
            return Err(invalid(&format!("el flujo {name} es demasiado grande")));
        }
        let data = if entry.size < 4096 {
            self.mini_chain(entry.start, entry.size)
        } else {
            self.chain(entry.start, Some(entry.size), MAX_PART_BYTES)
        };
        data.map(Some)
            .ok_or_else(|| invalid("documento de Office antiguo danado (flujo incompleto)"))
    }
}
