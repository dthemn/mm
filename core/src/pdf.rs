//! Text from PDFs that have a text layer, read on the device: bank statements and transfer confirmations.
//!
//! A page becomes lines of cells. Each piece of text is placed with the page's matrices, and its end is found
//! from the font's glyph widths, so pieces that touch join into one cell and a gap starts the next one.
//! Letters come from the font's `ToUnicode` map, or, when a PDF has none (Bank Millennium writes Polish
//! letters as bare glyph numbers), from the character map inside the embedded TrueType font, or else from
//! the font's encoding.

use lopdf::{Dictionary, Document, LoadOptions, Object, content::Content};
use std::collections::HashMap;

/// No real statement comes close: these only stop a crafted file from using all memory or time.
const MAX_PAGES: usize = 200;
const MAX_STREAM: usize = 32 * 1024 * 1024;
const MAX_PIECES: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    #[error("this file is not a PDF that can be read")]
    Unreadable,
    #[error("this PDF is protected with a password")]
    NeedPassword,
    #[error("the password is wrong")]
    WrongPassword,
}

/// Text that sits together on a line, starting at `x`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub x: f32,
    pub text: String,
}

/// One line of a page, top to bottom; `y` grows downwards from the top of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub y: f32,
    pub cells: Vec<Cell>,
}

impl Line {
    pub fn text(&self) -> String {
        self.cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// Every page's lines.
pub fn read(bytes: &[u8], password: Option<&str>) -> Result<Vec<Vec<Line>>, PdfError> {
    let doc = open(bytes, password)?;
    let mut pages = Vec::new();
    for (_, id) in doc.get_pages().into_iter().take(MAX_PAGES) {
        pages.push(page_lines(&doc, id));
    }
    Ok(pages)
}

fn open(bytes: &[u8], password: Option<&str>) -> Result<Document, PdfError> {
    if !bytes.starts_with(b"%PDF") && !bytes.get(..1024).is_some_and(|h| h.windows(4).any(|w| w == b"%PDF")) {
        return Err(PdfError::Unreadable);
    }
    let options = LoadOptions {
        password: password.map(str::to_string),
        max_decompressed_size: Some(MAX_STREAM),
        ..LoadOptions::default()
    };
    match Document::load_mem_with_options(bytes, options) {
        Ok(doc) if doc.is_encrypted() => Err(if password.is_some() { PdfError::WrongPassword } else { PdfError::NeedPassword }),
        Ok(doc) => Ok(doc),
        Err(lopdf::Error::Decryption(_)) | Err(lopdf::Error::InvalidPassword) => {
            Err(if password.is_some() { PdfError::WrongPassword } else { PdfError::NeedPassword })
        }
        Err(_) => Err(PdfError::Unreadable),
    }
}

type Matrix = [f32; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a` then `b`.
fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn translate(tx: f32, ty: f32) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, tx, ty]
}

fn num(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

fn nums(ops: &[Object]) -> Option<Matrix> {
    (ops.len() == 6).then(|| std::array::from_fn(|i| num(&ops[i])))
}

/// What a font needs to turn shown bytes into letters and widths.
struct Font {
    two_bytes: bool,
    unicode: HashMap<u32, String>,
    /// For simple fonts without a map: the encoding's letter per byte.
    simple: Option<[char; 256]>,
    widths: HashMap<u32, f32>,
    default_width: f32,
}

impl Font {
    fn codes<'a>(&self, s: &'a [u8]) -> Box<dyn Iterator<Item = u32> + 'a> {
        if self.two_bytes {
            Box::new(s.chunks(2).map(|c| u32::from(c[0]) << 8 | u32::from(*c.get(1).unwrap_or(&0))))
        } else {
            Box::new(s.iter().map(|b| u32::from(*b)))
        }
    }

    fn text(&self, code: u32) -> String {
        if let Some(t) = self.unicode.get(&code) {
            return t.clone();
        }
        match &self.simple {
            Some(table) if code < 256 => table[code as usize].to_string(),
            _ => '\u{fffd}'.to_string(),
        }
    }

    fn width(&self, code: u32) -> f32 {
        self.widths.get(&code).copied().unwrap_or(self.default_width) / 1000.0
    }
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    doc.dereference(o).map(|x| x.1).unwrap_or(o)
}

fn dict_get<'a>(doc: &'a Document, d: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    d.get(key).ok().map(|o| deref(doc, o))
}

fn stream_bytes(doc: &Document, o: &Object) -> Option<Vec<u8>> {
    match deref(doc, o) {
        Object::Stream(s) => s.decompressed_content_with_limit(MAX_STREAM).ok().or_else(|| Some(s.content.clone())),
        _ => None,
    }
}

fn load_font(doc: &Document, f: &Dictionary) -> Font {
    let subtype = dict_get(doc, f, b"Subtype").and_then(|o| o.as_name().ok()).unwrap_or(b"");
    let mut font = Font { two_bytes: false, unicode: HashMap::new(), simple: None, widths: HashMap::new(), default_width: 500.0 };
    if subtype == b"Type0" {
        font.two_bytes = true;
        font.default_width = 1000.0;
        let descendant = dict_get(doc, f, b"DescendantFonts").and_then(|o| o.as_array().ok())
            .and_then(|a| a.first()).map(|o| deref(doc, o)).and_then(|o| o.as_dict().ok());
        if let Some(d) = descendant {
            if let Some(w) = dict_get(doc, d, b"DW") {
                font.default_width = num(w);
            }
            if let Some(Object::Array(w)) = dict_get(doc, d, b"W") {
                cid_widths(doc, w, &mut font.widths);
            }
            let gid_of = cid_to_gid(doc, d);
            if let Some(glyphs) = descriptor_glyphs(doc, d) {
                // Glyph numbers in the PDF are CIDs; the font maps glyph ids, so go through CIDToGIDMap.
                match &gid_of {
                    None => font.unicode = glyphs.into_iter().map(|(g, c)| (u32::from(g), c.to_string())).collect(),
                    Some(map) => {
                        for (cid, gid) in map.iter().enumerate() {
                            if let Some(c) = glyphs.get(gid) {
                                font.unicode.insert(cid as u32, c.to_string());
                            }
                        }
                    }
                }
            }
        }
    } else {
        font.simple = Some(simple_table(doc, f));
        let first = dict_get(doc, f, b"FirstChar").map(num).unwrap_or(0.0) as u32;
        if let Some(Object::Array(w)) = dict_get(doc, f, b"Widths") {
            for (i, x) in w.iter().enumerate() {
                font.widths.insert(first + i as u32, num(deref(doc, x)));
            }
        }
        if let Some(name) = dict_get(doc, f, b"BaseFont").and_then(|o| o.as_name().ok())
            && font.widths.is_empty() && name.windows(7).any(|w| w == b"Courier")
        {
            font.default_width = 600.0;
        }
    }
    // An explicit map is the most reliable, so it wins over what the font file says.
    if let Some(bytes) = dict_get(doc, f, b"ToUnicode").and_then(|o| stream_bytes(doc, o)) {
        font.unicode.extend(parse_cmap(&bytes));
    }
    font
}

fn cid_widths(doc: &Document, w: &[Object], out: &mut HashMap<u32, f32>) {
    let mut i = 0;
    while i + 1 < w.len() {
        let first = num(deref(doc, &w[i])) as u32;
        match deref(doc, &w[i + 1]) {
            Object::Array(list) => {
                for (k, x) in list.iter().enumerate().take(65536) {
                    out.insert(first + k as u32, num(deref(doc, x)));
                }
                i += 2;
            }
            last if i + 2 < w.len() => {
                let (last, width) = (num(last) as u32, num(deref(doc, &w[i + 2])));
                for c in first..=last.min(first + 65535) {
                    out.insert(c, width);
                }
                i += 3;
            }
            _ => break,
        }
    }
}

/// `None` for Identity.
fn cid_to_gid(doc: &Document, d: &Dictionary) -> Option<Vec<u16>> {
    let bytes = dict_get(doc, d, b"CIDToGIDMap").and_then(|o| stream_bytes(doc, o))?;
    Some(bytes.chunks(2).map(|c| u16::from(c[0]) << 8 | u16::from(*c.get(1).unwrap_or(&0))).collect())
}

/// Glyph id to letter, from the embedded TrueType font's own character map.
fn descriptor_glyphs(doc: &Document, d: &Dictionary) -> Option<HashMap<u16, char>> {
    let desc = dict_get(doc, d, b"FontDescriptor")?.as_dict().ok()?;
    let data = dict_get(doc, desc, b"FontFile2").or_else(|| dict_get(doc, desc, b"FontFile3")).and_then(|o| stream_bytes(doc, o))?;
    let face = ttf_parser::Face::parse(&data, 0).ok()?;
    let cmap = face.tables().cmap?;
    let mut out = HashMap::new();
    for sub in cmap.subtables {
        if !sub.is_unicode() {
            continue;
        }
        sub.codepoints(|cp| {
            if let (Some(g), Some(c)) = (sub.glyph_index(cp), char::from_u32(cp)) {
                out.entry(g.0).or_insert(c);
            }
        });
    }
    Some(out)
}

/// The 256 letters of a simple font: its base encoding, then its Differences.
fn simple_table(doc: &Document, f: &Dictionary) -> [char; 256] {
    let mut base: &[u8] = b"WinAnsiEncoding";
    let mut diffs: Option<&Vec<Object>> = None;
    match dict_get(doc, f, b"Encoding") {
        Some(Object::Name(n)) => base = n,
        Some(Object::Dictionary(d)) => {
            if let Some(Object::Name(n)) = dict_get(doc, d, b"BaseEncoding") {
                base = n;
            }
            if let Some(Object::Array(a)) = dict_get(doc, d, b"Differences") {
                diffs = Some(a);
            }
        }
        _ => {}
    }
    let enc = if base == b"MacRomanEncoding" { encoding_rs::MACINTOSH } else { encoding_rs::WINDOWS_1252 };
    let mut table = ['\u{fffd}'; 256];
    for (b, slot) in table.iter_mut().enumerate() {
        let byte = [b as u8];
        let (s, _) = enc.decode_without_bom_handling(&byte);
        *slot = s.chars().next().filter(|c| !c.is_control() || *c == '\t').unwrap_or('\u{fffd}');
    }
    if let Some(a) = diffs {
        let mut code = 0usize;
        for o in a {
            match deref(doc, o) {
                Object::Integer(i) => code = (*i).clamp(0, 255) as usize,
                Object::Name(n) => {
                    if code < 256 && let Some(c) = glyph_name(n) {
                        table[code] = c;
                    }
                    code += 1;
                }
                _ => {}
            }
        }
    }
    table
}

/// Letters by glyph name: the Polish ones and the `uniXXXX` form; the rest keep the base encoding.
fn glyph_name(n: &[u8]) -> Option<char> {
    let n = std::str::from_utf8(n).ok()?;
    if let Some(hex) = n.strip_prefix("uni").filter(|h| h.len() == 4) {
        return char::from_u32(u32::from_str_radix(hex, 16).ok()?);
    }
    Some(match n {
        "space" => ' ',
        "aogonek" => 'ą', "Aogonek" => 'Ą', "cacute" => 'ć', "Cacute" => 'Ć', "eogonek" => 'ę', "Eogonek" => 'Ę',
        "lslash" => 'ł', "Lslash" => 'Ł', "nacute" => 'ń', "Nacute" => 'Ń', "oacute" => 'ó', "Oacute" => 'Ó',
        "sacute" => 'ś', "Sacute" => 'Ś', "zacute" => 'ź', "Zacute" => 'Ź', "zdotaccent" => 'ż', "Zdotaccent" => 'Ż',
        _ => return None,
    })
}

fn hex(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits.chunks(2).filter_map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok().map(|b| if c.len() == 1 { b << 4 } else { b })).collect()
}

fn code_of(b: &[u8]) -> u32 {
    b.iter().take(4).fold(0, |a, x| a << 8 | u32::from(*x))
}

fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks(2).map(|c| u16::from(c[0]) << 8 | u16::from(*c.get(1).unwrap_or(&0))).collect();
    String::from_utf16_lossy(&units)
}

/// The `bfchar` and `bfrange` entries of a ToUnicode CMap.
fn parse_cmap(bytes: &[u8]) -> HashMap<u32, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = HashMap::new();
    let tokens: Vec<&str> = text.split(|c: char| c.is_whitespace()).filter(|t| !t.is_empty()).collect();
    // Tokens split on whitespace keep `<..>` and `[`/`]` attached; re-split them into single items.
    let mut items: Vec<String> = Vec::new();
    for t in tokens {
        let mut cur = String::new();
        for ch in t.chars() {
            match ch {
                '<' | '[' => {
                    if !cur.is_empty() { items.push(std::mem::take(&mut cur)); }
                    if ch == '[' { items.push("[".into()); } else { cur.push('<'); }
                }
                '>' => {
                    cur.push('>');
                    items.push(std::mem::take(&mut cur));
                }
                ']' => {
                    if !cur.is_empty() { items.push(std::mem::take(&mut cur)); }
                    items.push("]".into());
                }
                _ => cur.push(ch),
            }
        }
        if !cur.is_empty() { items.push(cur); }
    }
    let is_hex = |s: &str| s.starts_with('<') && s.ends_with('>') && !s.starts_with("<<");
    let mut i = 0;
    let mut mode = "";
    while i < items.len() && out.len() < 70_000 {
        match items[i].as_str() {
            "beginbfchar" => { mode = "char"; i += 1; continue; }
            "beginbfrange" => { mode = "range"; i += 1; continue; }
            "endbfchar" | "endbfrange" => { mode = ""; i += 1; continue; }
            _ => {}
        }
        if mode == "char" && i + 1 < items.len() && is_hex(&items[i]) && is_hex(&items[i + 1]) {
            out.insert(code_of(&hex(&items[i])), utf16(&hex(&items[i + 1])));
            i += 2;
        } else if mode == "range" && i + 2 < items.len() && is_hex(&items[i]) && is_hex(&items[i + 1]) {
            let (lo, hi) = (code_of(&hex(&items[i])), code_of(&hex(&items[i + 1])));
            if items[i + 2] == "[" {
                let mut j = i + 3;
                let mut c = lo;
                while j < items.len() && items[j] != "]" {
                    if c <= hi { out.insert(c, utf16(&hex(&items[j]))); }
                    c += 1;
                    j += 1;
                }
                i = j + 1;
            } else if is_hex(&items[i + 2]) {
                let start = hex(&items[i + 2]);
                for (k, c) in (lo..=hi.min(lo + 65535)).enumerate() {
                    let mut dst = start.clone();
                    // The last byte counts up across the range.
                    if let Some(last) = dst.last_mut() {
                        *last = last.wrapping_add(k as u8);
                    }
                    out.insert(c, utf16(&dst));
                }
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Text shown in one go, in page coordinates with y growing downwards.
#[derive(Debug)]
struct Piece {
    x: f32,
    end: f32,
    y: f32,
    size: f32,
    text: String,
}

#[derive(Clone)]
struct Gs {
    ctm: Matrix,
    font: Option<Vec<u8>>,
    size: f32,
    char_space: f32,
    word_space: f32,
    scale: f32,
    leading: f32,
}

fn page_lines(doc: &Document, page: lopdf::ObjectId) -> Vec<Line> {
    let fonts: HashMap<Vec<u8>, Font> = doc.get_page_fonts(page).map(|m| {
        m.into_iter().map(|(name, d)| (name, load_font(doc, d))).collect()
    }).unwrap_or_default();
    let Ok(content) = doc.get_page_content_with_limit(page, MAX_STREAM) else { return vec![] };
    let Ok(content) = Content::decode(&content) else { return vec![] };
    let height = page_height(doc, page);

    let mut pieces: Vec<Piece> = Vec::new();
    let mut gs = Gs { ctm: IDENTITY, font: None, size: 0.0, char_space: 0.0, word_space: 0.0, scale: 1.0, leading: 0.0 };
    let mut stack: Vec<Gs> = Vec::new();
    let (mut tm, mut tlm) = (IDENTITY, IDENTITY);

    for op in &content.operations {
        if pieces.len() >= MAX_PIECES {
            break;
        }
        let o = &op.operands;
        match op.operator.as_str() {
            "q" => if stack.len() < 64 { stack.push(gs.clone()) },
            "Q" => if let Some(g) = stack.pop() { gs = g },
            "cm" => if let Some(m) = nums(o) { gs.ctm = mul(&m, &gs.ctm) },
            "BT" => { tm = IDENTITY; tlm = IDENTITY; }
            "Tf" if o.len() == 2 => {
                gs.font = o[0].as_name().ok().map(<[u8]>::to_vec);
                gs.size = num(&o[1]);
            }
            "Tc" if !o.is_empty() => gs.char_space = num(&o[0]),
            "Tw" if !o.is_empty() => gs.word_space = num(&o[0]),
            "Tz" if !o.is_empty() => gs.scale = num(&o[0]) / 100.0,
            "TL" if !o.is_empty() => gs.leading = num(&o[0]),
            "Td" | "TD" if o.len() == 2 => {
                let (tx, ty) = (num(&o[0]), num(&o[1]));
                if op.operator == "TD" { gs.leading = -ty; }
                tlm = mul(&translate(tx, ty), &tlm);
                tm = tlm;
            }
            "Tm" => if let Some(m) = nums(o) { tlm = m; tm = m; },
            "T*" => { tlm = mul(&translate(0.0, -gs.leading), &tlm); tm = tlm; }
            "Tj" | "'" | "\"" | "TJ" => {
                if op.operator == "'" || op.operator == "\"" {
                    if op.operator == "\"" && o.len() == 3 {
                        gs.word_space = num(&o[0]);
                        gs.char_space = num(&o[1]);
                    }
                    tlm = mul(&translate(0.0, -gs.leading), &tlm);
                    tm = tlm;
                }
                let Some(font) = gs.font.as_ref().and_then(|n| fonts.get(n)) else { continue };
                let items: Vec<&Object> = match op.operator.as_str() {
                    "TJ" => match o.first() { Some(Object::Array(a)) => a.iter().collect(), _ => vec![] },
                    _ => o.last().into_iter().collect(),
                };
                show(&items, font, &gs, &mut tm, height, &mut pieces);
            }
            _ => {}
        }
    }
    assemble(pieces)
}

fn page_height(doc: &Document, page: lopdf::ObjectId) -> f32 {
    let mut id = Some(page);
    for _ in 0..32 {
        let Some(d) = id.and_then(|i| doc.get_dictionary(i).ok()) else { break };
        if let Some(Object::Array(b)) = dict_get(doc, d, b"MediaBox") && b.len() == 4 {
            return num(deref(doc, &b[3])) - num(deref(doc, &b[1]));
        }
        id = d.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    842.0
}

/// Shows strings and number adjustments, one piece per run of glyphs.
fn show(items: &[&Object], font: &Font, gs: &Gs, tm: &mut Matrix, height: f32, out: &mut Vec<Piece>) {
    let start = |tm: &Matrix| {
        let m = mul(tm, &gs.ctm);
        (m[4], height - m[5], (m[2] * m[2] + m[3] * m[3]).sqrt() * gs.size)
    };
    for item in items {
        match item {
            Object::String(s, _) => {
                let (x, y, size) = start(tm);
                let mut text = String::new();
                let mut advance = 0.0;
                for code in font.codes(s) {
                    text.push_str(&font.text(code));
                    let space = if !font.two_bytes && code == 32 { gs.word_space } else { 0.0 };
                    advance += (font.width(code) * gs.size + gs.char_space + space) * gs.scale;
                }
                *tm = mul(&translate(advance, 0.0), tm);
                let (end, _, _) = start(tm);
                if !text.is_empty() {
                    out.push(Piece { x: x.min(end), end: x.max(end), y, size: size.abs().max(1.0), text });
                }
            }
            other => {
                let adj = num(other);
                *tm = mul(&translate(-adj / 1000.0 * gs.size * gs.scale, 0.0), tm);
            }
        }
    }
}

/// Pieces into lines, and touching pieces into cells.
fn assemble(mut pieces: Vec<Piece>) -> Vec<Line> {
    pieces.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut lines: Vec<(f32, f32, Vec<Piece>)> = Vec::new();
    for p in pieces {
        match lines.last_mut() {
            Some((y, size, row)) if (p.y - *y).abs() <= 0.4 * size.min(p.size) => {
                *size = size.max(p.size);
                row.push(p);
            }
            _ => lines.push((p.y, p.size, vec![p])),
        }
    }
    let mut out = Vec::new();
    for (y, _, mut row) in lines {
        row.sort_by(|a, b| a.x.total_cmp(&b.x));
        let mut cells: Vec<(f32, f32, f32, String)> = Vec::new();
        for p in row {
            match cells.last_mut() {
                // A small gap is a word space the PDF drew as a move; a big one is the next column.
                Some((_, end, size, text)) if p.x - *end < 0.6 * *size => {
                    if p.x - *end > 0.15 * *size && !text.ends_with(' ') && !p.text.starts_with(' ') {
                        text.push(' ');
                    }
                    text.push_str(&p.text);
                    *end = end.max(p.end);
                }
                _ => cells.push((p.x, p.end, p.size, p.text)),
            }
        }
        let cells: Vec<Cell> = cells.into_iter()
            .map(|(x, _, _, t)| Cell { x, text: t.split_whitespace().collect::<Vec<_>>().join(" ") })
            .filter(|c| !c.text.is_empty())
            .collect();
        if !cells.is_empty() {
            out.push(Line { y, cells });
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lopdf::{Stream, dictionary};

    /// A PDF with the given lines of cells, as `(x, y from the top, text)`. Latin text uses Helvetica; text
    /// with other letters uses a two-byte font with a ToUnicode map, as many banks write it.
    pub fn make_pdf(cells: &[(f32, f32, &str)]) -> Vec<u8> {
        make_pdf_locked(cells, None)
    }

    /// The same, protected with a password when one is given.
    pub fn make_pdf_locked(cells: &[(f32, f32, &str)], password: Option<&str>) -> Vec<u8> {
        let mut doc = Document::with_version("1.7");
        let mut glyphs: Vec<char> = Vec::new();
        let mut ops = String::new();
        for (x, y, text) in cells {
            let latin = text.is_ascii();
            ops.push_str(&format!("BT 1 0 0 1 {x} {} Tm ", 842.0 - y));
            if latin {
                let esc = text.replace('\\', "\\\\").replace('(', "\\(").replace(')', "\\)");
                ops.push_str(&format!("/F1 9 Tf ({esc}) Tj ET\n"));
            } else {
                let mut hexs = String::new();
                for c in text.chars() {
                    let g = match glyphs.iter().position(|x| *x == c) { Some(i) => i, None => { glyphs.push(c); glyphs.len() - 1 } } + 1;
                    hexs.push_str(&format!("{g:04X}"));
                }
                ops.push_str(&format!("/F2 9 Tf <{hexs}> Tj ET\n"));
            }
        }
        let mut cmap = String::from("/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n1 begincodespacerange <0000> <FFFF> endcodespacerange\n");
        cmap.push_str(&format!("{} beginbfchar\n", glyphs.len()));
        for (i, c) in glyphs.iter().enumerate() {
            let mut b = [0u16; 2];
            let units: String = c.encode_utf16(&mut b).iter().map(|u| format!("{u:04X}")).collect();
            cmap.push_str(&format!("<{:04X}> <{units}>\n", i + 1));
        }
        cmap.push_str("endbfchar endcmap CMapName currentdict /CMap defineresource pop end end\n");
        let to_unicode = doc.add_object(Stream::new(dictionary! {}, cmap.into_bytes()));
        let f1 = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" });
        let cid = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "Test",
            "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
            "DW" => 550,
        });
        let f2 = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Test", "Encoding" => "Identity-H",
            "DescendantFonts" => vec![cid.into()], "ToUnicode" => to_unicode,
        });
        let content = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
        let pages = doc.new_object_id();
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => f1, "F2" => f2 } },
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        doc.compress();
        if let Some(pw) = password {
            let id = Object::string_literal("mm-test-file-id!");
            doc.trailer.set("ID", vec![id.clone(), id]);
            let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
                document: &doc, owner_password: "owner", user_password: pw, key_length: 128,
                permissions: lopdf::Permissions::all(),
            }).unwrap();
            doc.encrypt(&state).unwrap();
        }
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }

    #[test]
    fn lines_cells_and_polish_letters() {
        let pdf = make_pdf(&[(40.0, 100.0, "Odbiorca"), (250.0, 100.0, "Łódź Żółć"), (40.0, 120.0, "Kwota"), (250.0, 120.0, "12,50 PLN")]);
        let pages = read(&pdf, None).unwrap();
        assert_eq!(pages.len(), 1);
        let l = &pages[0];
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["Odbiorca", "Łódź Żółć"]);
        assert_eq!(l[1].text(), "Kwota 12,50 PLN");
        assert!(l[0].y < l[1].y, "top first");
    }

    #[test]
    fn touching_pieces_join_and_far_ones_split() {
        // The test font has no widths, so each letter counts as half its size: "Ab" at 9 pt ends at 49.
        let pdf = make_pdf(&[(40.0, 50.0, "Ab"), (49.2, 50.0, "cd"), (60.0, 50.0, "ef"), (200.0, 50.0, "far")]);
        let l = &read(&pdf, None).unwrap()[0];
        assert_eq!(l[0].cells.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["Abcd ef", "far"]);
    }

    #[test]
    fn cmap_ranges() {
        let m = parse_cmap(b"2 beginbfrange <0010> <0012> <0041> <0020> <0021> [<0105> <0142>] endbfrange 1 beginbfchar <0001><D83DDE00> endbfchar");
        assert_eq!(m[&0x10], "A");
        assert_eq!(m[&0x12], "C");
        assert_eq!(m[&0x20], "ą");
        assert_eq!(m[&0x21], "ł");
        assert_eq!(m[&0x01], "😀");
    }

    #[test]
    fn a_password_is_asked_for_and_checked() {
        let pdf = make_pdf_locked(&[(40.0, 50.0, "Saldo"), (200.0, 50.0, "Żółw")], Some("tajne"));
        assert_eq!(read(&pdf, None), Err(PdfError::NeedPassword));
        assert_eq!(read(&pdf, Some("wrong")), Err(PdfError::WrongPassword));
        assert_eq!(read(&pdf, Some("tajne")).unwrap()[0][0].text(), "Saldo Żółw");
    }

    #[test]
    fn not_a_pdf_and_garbage() {
        assert_eq!(read(b"hello", None), Err(PdfError::Unreadable));
        assert_eq!(read(b"%PDF-1.7\n garbage", None), Err(PdfError::Unreadable));
        let mut pdf = make_pdf(&[(40.0, 50.0, "x")]);
        pdf.truncate(pdf.len() / 2);
        let _ = read(&pdf, None);
    }
}
