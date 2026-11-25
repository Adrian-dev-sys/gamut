//! Read-only reader for LiteDB v4 files (what Playnite stores its library in).
//! Walks data pages and decodes every document; indexes are ignored.
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

const PAGE: usize = 4096;
const HEADER: usize = 25;
const DATA_PAGE: u8 = 4;
const NONE: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(String),
    /// Milliseconds since the Unix epoch (UTC).
    Date(i64),
    Doc(Doc),
    Array(Vec<Value>),
    Bytes(Vec<u8>),
}

pub type Doc = BTreeMap<String, Value>;

impl Value {
    pub fn str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Double(d) => Some(*d as i64),
            _ => None,
        }
    }
    pub fn bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }
    pub fn date_ms(&self) -> Option<i64> {
        match self {
            Value::Date(d) => Some(*d),
            _ => None,
        }
    }
    pub fn array(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }
    pub fn doc(&self) -> Option<&Doc> {
        match self {
            Value::Doc(d) => Some(d),
            _ => None,
        }
    }
}

/// Reads every document in a LiteDB v4 file.
pub fn read(path: &Path) -> Result<Vec<Doc>> {
    let f = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    parse(&f).with_context(|| format!("parsing {}", path.display()))
}

pub fn parse(f: &[u8]) -> Result<Vec<Doc>> {
    const MAGIC: &[u8] = b"** This is a LiteDB file **";
    if f.len() < PAGE || f.get(HEADER..HEADER + MAGIC.len()) != Some(MAGIC) {
        bail!("not a LiteDB v4 file");
    }
    let pages = f.len() / PAGE;
    let hdr = |p: usize| -> Option<(u8, u32, u16, u16)> {
        let o = p.checked_mul(PAGE)?;
        let h = f.get(o..o + HEADER)?;
        Some((h[4], u32le(&h[9..13]), u16le(&h[13..15]), u16le(&h[15..17])))
    };
    let mut out = Vec::new();
    for p in 0..pages {
        let Some((DATA_PAGE, _, count, _)) = hdr(p) else { continue };
        let mut o = p * PAGE + HEADER;
        let end = (p + 1) * PAGE;
        for _ in 0..count {
            let Some(b) = f.get(o..o + 8) else { break };
            let (ext, size) = (u32le(&b[2..6]), u16le(&b[6..8]) as usize);
            o += 8;
            if o + size > end {
                bail!("data block overruns page {p}");
            }
            let mut data = f[o..o + size].to_vec();
            o += size;
            // Large documents live in a chain of extend pages instead.
            if ext != NONE {
                data.clear();
                let (mut e, mut hops) = (ext as usize, 0);
                while e != NONE as usize {
                    let (_, next, _, free) = hdr(e).context("extend page out of range")?;
                    let s = e * PAGE + HEADER;
                    let len = (PAGE - HEADER).saturating_sub(free as usize);
                    data.extend_from_slice(&f[s..s + len]);
                    e = next as usize;
                    hops += 1;
                    if hops > pages {
                        bail!("extend page loop");
                    }
                }
            }
            out.push(doc(&data, &mut 0)?);
        }
    }
    Ok(out)
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn take<'a>(b: &'a [u8], i: &mut usize, n: usize) -> Result<&'a [u8]> {
    let s = b.get(*i..*i + n).context("truncated document")?;
    *i += n;
    Ok(s)
}

fn doc(b: &[u8], i: &mut usize) -> Result<Doc> {
    let start = *i;
    let len = i32::from_le_bytes(take(b, i, 4)?.try_into()?) as usize;
    let end = start + len - 1;
    let mut d = Doc::new();
    while *i < end {
        let t = take(b, i, 1)?[0];
        let nul = b[*i..].iter().position(|&c| c == 0).context("unterminated key")?;
        let key = String::from_utf8_lossy(&b[*i..*i + nul]).into_owned();
        *i += nul + 1;
        let v = match t {
            0x01 => Value::Double(f64::from_le_bytes(take(b, i, 8)?.try_into()?)),
            0x02 => {
                let n = i32::from_le_bytes(take(b, i, 4)?.try_into()?) as usize;
                let s = take(b, i, n)?;
                Value::Str(String::from_utf8_lossy(&s[..n.saturating_sub(1)]).into_owned())
            }
            0x03 => Value::Doc(doc(b, i)?),
            0x04 => Value::Array(doc(b, i)?.into_values().collect()),
            0x05 => {
                let n = i32::from_le_bytes(take(b, i, 4)?.try_into()?) as usize;
                let sub = take(b, i, 1)?[0];
                let raw = take(b, i, n)?;
                if (sub == 3 || sub == 4) && n == 16 {
                    Value::Str(guid(raw))
                } else {
                    Value::Bytes(raw.to_vec())
                }
            }
            0x07 => Value::Bytes(take(b, i, 12)?.to_vec()),
            0x08 => Value::Bool(take(b, i, 1)?[0] != 0),
            0x09 => Value::Date(i64::from_le_bytes(take(b, i, 8)?.try_into()?)),
            0x0A => Value::Null,
            0x10 => Value::Int(i32::from_le_bytes(take(b, i, 4)?.try_into()?) as i64),
            0x12 => Value::Int(i64::from_le_bytes(take(b, i, 8)?.try_into()?)),
            0x13 => Value::Bytes(take(b, i, 16)?.to_vec()),
            0x7F | 0xFF => Value::Null,
            _ => bail!("unsupported BSON type {t:#x}"),
        };
        d.insert(key, v);
    }
    *i = end + 1;
    Ok(d)
}

/// .NET GUID byte order (first three groups little-endian), lowercase.
fn guid(r: &[u8]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        r[3], r[2], r[1], r[0], r[5], r[4], r[7], r[6], r[8], r[9], r[10], r[11], r[12], r[13], r[14], r[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bson(fields: &[(u8, &str, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (t, k, v) in fields {
            body.push(*t);
            body.extend_from_slice(k.as_bytes());
            body.push(0);
            body.extend_from_slice(v);
        }
        let mut d = ((body.len() + 5) as i32).to_le_bytes().to_vec();
        d.extend(body);
        d.push(0);
        d
    }
    fn bstr(s: &str) -> Vec<u8> {
        let mut v = ((s.len() + 1) as i32).to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        v
    }
    fn page(id: u32, typ: u8, next: u32, items: u16, free: u16, body: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8; PAGE];
        p[0..4].copy_from_slice(&id.to_le_bytes());
        p[4] = typ;
        p[5..9].copy_from_slice(&NONE.to_le_bytes());
        p[9..13].copy_from_slice(&next.to_le_bytes());
        p[13..15].copy_from_slice(&items.to_le_bytes());
        p[15..17].copy_from_slice(&free.to_le_bytes());
        p[HEADER..HEADER + body.len()].copy_from_slice(body);
        p
    }

    #[test]
    fn reads_inline_and_extended_documents() {
        let mut header = page(0, 1, NONE, 0, 0, b"** This is a LiteDB file **");
        header[52] = 7;
        let small = bson(&[(0x02, "Name", bstr("Pokémon")), (0x10, "Playtime", 90i32.to_le_bytes().to_vec())]);
        let big_desc = "x".repeat(5000);
        let big = bson(&[(0x02, "Description", bstr(&big_desc)), (0x09, "Added", 1_700_000_000_000i64.to_le_bytes().to_vec())]);

        // Data page: one inline block, one block pointing at extend pages 2 -> 3.
        let mut body = Vec::new();
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&NONE.to_le_bytes());
        body.extend_from_slice(&(small.len() as u16).to_le_bytes());
        body.extend_from_slice(&small);
        body.extend_from_slice(&1u16.to_le_bytes());
        body.extend_from_slice(&2u32.to_le_bytes());
        body.extend_from_slice(&0u16.to_le_bytes());
        let cap = PAGE - HEADER;
        let (a, b) = big.split_at(cap);
        let mut file = header;
        file.extend(page(1, DATA_PAGE, NONE, 2, 0, &body));
        file.extend(page(2, 5, 3, 0, 0, a));
        file.extend(page(3, 5, NONE, 0, (cap - b.len()) as u16, b));

        let docs = parse(&file).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0]["Name"].str(), Some("Pokémon"));
        assert_eq!(docs[0]["Playtime"].int(), Some(90));
        assert_eq!(docs[1]["Description"].str().map(str::len), Some(5000));
        assert_eq!(docs[1]["Added"].date_ms(), Some(1_700_000_000_000));
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(&vec![0u8; PAGE]).is_err());
    }
}
