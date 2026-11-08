//! Lector XML minimo, de tipo "pull": devuelve eventos (etiqueta de inicio,
//! etiqueta de cierre, texto) a medida que recorre el documento.
//!
//! Es deliberadamente simple y seguro:
//! - No procesa DTD ni entidades definidas por el documento. Eso evita los
//!   ataques "billion laughs" (entidades que se expanden a gigabytes) y XXE
//!   (entidades que leen archivos del disco o de la red). Solo se traducen
//!   las cinco entidades estandar (`&lt;` `&gt;` `&amp;` `&quot;` `&apos;`) y
//!   las referencias numericas (`&#233;`, `&#xE9;`).
//! - No valida el documento: si esta mal formado, se extrae lo que se pueda.
//! - Trabaja sobre el texto ya cargado, sin recursion.

use std::borrow::Cow;

pub enum Event<'a> {
    /// `<nombre atributos>` o `<nombre atributos/>` (`empty = true`).
    Start {
        name: &'a str,
        attrs: &'a str,
        empty: bool,
    },
    /// `</nombre>`
    End { name: &'a str },
    /// Texto entre etiquetas, con las entidades ya traducidas.
    Text(Cow<'a, str>),
}

pub struct Reader<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(text: &'a str) -> Self {
        Reader { text, pos: 0 }
    }

    /// Avanza hasta despues de `marker`, o hasta el final si no aparece.
    fn skip_past(&mut self, marker: &str) {
        self.pos = match self.text[self.pos..].find(marker) {
            Some(index) => self.pos + index + marker.len(),
            None => self.text.len(),
        };
    }
}

/// Posicion del `>` que cierra una etiqueta, ignorando los `>` que haya
/// dentro de valores de atributos entre comillas.
fn tag_end(rest: &str) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (index, &byte) in rest.as_bytes().iter().enumerate() {
        match (quote, byte) {
            (None, b'"' | b'\'') => quote = Some(byte),
            (Some(q), b) if b == q => quote = None,
            (None, b'>') => return Some(index),
            _ => {}
        }
    }
    None
}

impl<'a> Iterator for Reader<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        loop {
            let rest = self.text.get(self.pos..)?;
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(Event::Text(unescape(&rest[..end])));
            }
            if rest.starts_with("<!--") {
                self.skip_past("-->");
                continue;
            }
            if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
                let end = cdata.find("]]>").unwrap_or(cdata.len());
                self.pos += 9 + end + 3.min(cdata.len() - end);
                return Some(Event::Text(Cow::Borrowed(&cdata[..end])));
            }
            if rest.starts_with("<?") {
                self.skip_past("?>");
                continue;
            }
            if rest.starts_with("<!") {
                // DOCTYPE: se salta entero, sin interpretar sus entidades.
                let close = rest.find('>').unwrap_or(rest.len());
                match rest[..close].find('[') {
                    Some(_) => self.skip_past("]>"),
                    None => self.pos += (close + 1).min(rest.len()),
                }
                continue;
            }

            let Some(end) = tag_end(rest) else {
                self.pos = self.text.len();
                return None;
            };
            self.pos += end + 1;
            let tag = &rest[1..end];
            if let Some(name) = tag.strip_prefix('/') {
                return Some(Event::End { name: name.trim() });
            }
            let (tag, empty) = match tag.strip_suffix('/') {
                Some(tag) => (tag, true),
                None => (tag, false),
            };
            let split = tag
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(tag.len());
            return Some(Event::Start {
                name: &tag[..split],
                attrs: &tag[split..],
                empty,
            });
        }
    }
}

/// Nombre sin prefijo de espacio de nombres: `w:t` -> `t`.
pub fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Recorre los atributos de una etiqueta como pares (nombre completo, valor
/// sin traducir entidades).
pub fn attrs(text: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        let eq = rest.find('=')?;
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let value_end = after[1..].find(quote)?;
        let value = &after[1..1 + value_end];
        rest = &after[1 + value_end + 1..];
        Some((name, value))
    })
}

/// Valor de un atributo sin prefijo de espacio de nombres (`name="..."`).
pub fn attr<'a>(text: &'a str, wanted: &str) -> Option<Cow<'a, str>> {
    attrs(text)
        .find(|(name, _)| local(name) == wanted)
        .map(|(_, value)| unescape(value))
}

/// Valor de un atributo con prefijo cuyo nombre local es `wanted`
/// (por ejemplo `r:id`, sin importar que prefijo use el documento).
pub fn prefixed_attr<'a>(text: &'a str, wanted: &str) -> Option<Cow<'a, str>> {
    attrs(text)
        .find(|(name, _)| name.contains(':') && local(name) == wanted)
        .map(|(_, value)| unescape(value))
}

/// Traduce las entidades estandar y las referencias numericas.
pub fn unescape(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // Se busca el `;` solo en los primeros bytes (las entidades son
        // cortas). Se recorren bytes para no cortar un caracter UTF-8.
        let Some(semi) = rest.bytes().take(12).position(|byte| byte == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity.strip_prefix('#').and_then(|number| {
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse::<u32>().ok(),
                };
                code.and_then(char::from_u32)
            }),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}
