//! API keys in config.toml, encrypted for the current Windows user (DPAPI) as `dpapi:<hex>`.
//! Plain values still load (older configs) and are encrypted on the next save.
use serde::{Deserialize, Deserializer, Serializer};

const PREFIX: &str = "dpapi:";

pub fn serialize<S: Serializer>(v: &str, s: S) -> Result<S::Ok, S::Error> {
    if v.is_empty() {
        return s.serialize_str("");
    }
    match protect(v.as_bytes()) {
        Some(b) => s.serialize_str(&format!("{PREFIX}{}", hex(&b))),
        // Never lose a key: plain is better than nothing.
        None => s.serialize_str(v),
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = String::deserialize(d)?;
    let Some(h) = v.strip_prefix(PREFIX) else { return Ok(v) };
    // Another Windows user or PC (e.g. a restored backup) can't read it.
    Ok(unhex(h).and_then(|b| unprotect(&b)).and_then(|b| String::from_utf8(b).ok()).unwrap_or_else(|| {
        log::warn!("an API key in config.toml can't be decrypted by this Windows user; enter it again");
        String::new()
    }))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

#[cfg(windows)]
fn crypt(data: &[u8], encrypt: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        if encrypt {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData as _) };
    Some(v)
}

#[cfg(not(windows))]
fn crypt(_: &[u8], _: bool) -> Option<Vec<u8>> {
    None
}

fn protect(b: &[u8]) -> Option<Vec<u8>> {
    crypt(b, true)
}

fn unprotect(b: &[u8]) -> Option<Vec<u8>> {
    crypt(b, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_plain_and_foreign() {
        let enc = protect(b"abc123").unwrap();
        assert_ne!(enc, b"abc123");
        assert_eq!(unprotect(&enc).unwrap(), b"abc123");
        assert_eq!(unhex(&hex(&enc)).unwrap(), enc);
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        struct K {
            #[serde(with = "super")]
            k: String,
        }
        let text = toml::to_string(&K { k: "SECRET".into() }).unwrap();
        assert!(text.contains("dpapi:") && !text.contains("SECRET"), "{text}");
        assert_eq!(toml::from_str::<K>(&text).unwrap().k, "SECRET");
        assert_eq!(toml::from_str::<K>("k = \"plain\"").unwrap().k, "plain");
        assert_eq!(toml::from_str::<K>("k = \"dpapi:00ff\"").unwrap().k, "");
        assert_eq!(toml::to_string(&K { k: String::new() }).unwrap().trim(), "k = \"\"");
    }
}
