//! Pairing by typing instead of scanning: a one-time ID and a 6-digit code, like "K7Q4-MX2T" and
//! "493812". Run through scrypt, the two give a throwaway identity, and the machine publishes a
//! pkarr record under it naming its real id and addresses.

use serde::{Deserialize, Serialize};

use crate::identity::Identity;
use crate::js;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortCode {
    /// 8 characters of Crockford base32, without the dash
    pub id: String,
    /// 6 digits
    pub code: String,
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const SCRYPT_SALT: &str = "inyon-pair-id:v2";

pub fn new_short_code() -> Result<ShortCode, getrandom::Error> {
    let mut id = [0u8; 8];
    getrandom::fill(&mut id)?;
    let mut code = String::with_capacity(6);
    while code.len() < 6 {
        let mut b = [0u8; 6];
        getrandom::fill(&mut b)?;
        // 250 = 25 * 10: rejecting the rest keeps every digit equally likely
        for b in b {
            if b < 250 && code.len() < 6 {
                code.push((b'0' + b % 10) as char);
            }
        }
    }
    Ok(ShortCode { id: id.iter().map(|b| CROCKFORD[(b & 31) as usize] as char).collect(), code })
}

/// "K7Q4MX2T" → "K7Q4-MX2T"
pub fn format_short_id(id: &str) -> String {
    let at = id.char_indices().nth(4).map_or(id.len(), |(at, _)| at);
    format!("{}-{}", &id[..at], &id[at..])
}

/// The secret the phone sends: the ID and code together, so a code only works with its own ID.
pub fn short_secret(c: &ShortCode) -> String {
    format!("{}:{}", c.id, c.code)
}

/// People mix up 0/O and 1/I/L when copying an ID off a terminal.
fn clean_id(s: &str) -> String {
    s.chars()
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            'U' => 'V',
            c => c,
        })
        .collect()
}

/// Reads "K7Q4-MX2T 493812" (in any spacing or case); None if it isn't an ID and code.
pub fn parse_short_code(text: &str) -> Option<ShortCode> {
    // the TS side's /^\s*(\w{4})[\s-]*(\w{4})[\s:,-]*(\d{3})[\s-]*(\d{3})\s*$/ (alphanumerics, not \w);
    // the groups have fixed lengths and the separators never overlap them, so reading left to right
    // finds the only possible match
    let mut chars = text.chars().peekable();
    let mut take = |n: usize, ok: fn(char) -> bool, sep: fn(char) -> bool| -> Option<String> {
        while chars.next_if(|&c| sep(c)).is_some() {}
        let mut out = String::with_capacity(n);
        for _ in 0..n {
            out.push(chars.next_if(|&c| ok(c))?);
        }
        Some(out)
    };
    let alnum = |c: char| c.is_ascii_alphanumeric();
    let digit = |c: char| c.is_ascii_digit();
    let a = take(4, alnum, js::is_space)?;
    let b = take(4, alnum, |c| js::is_space(c) || c == '-')?;
    let c = take(3, digit, |c| js::is_space(c) || matches!(c, ':' | ',' | '-'))?;
    let d = take(3, digit, |c| js::is_space(c) || c == '-')?;
    while chars.next_if(|&c| js::is_space(c)).is_some() {}
    if chars.next().is_some() {
        return None;
    }
    let id = clean_id(&(a + &b));
    if !id.bytes().all(|b| CROCKFORD.contains(&b)) {
        return None;
    }
    Some(ShortCode { id, code: c + &d })
}

/// The throwaway identity an ID and code stand for: scrypt(N=16384, r=8, p=1) gives its seed, which
/// takes 16 MiB and tens of milliseconds a try, so going through the million codes behind an ID is
/// real work on top of the relay lookups.
pub fn short_code_identity(c: &ShortCode) -> Identity {
    let params = scrypt::Params::new(14, 8, 1, 32).expect("fixed scrypt parameters are valid");
    let mut seed = [0u8; 32];
    scrypt::scrypt(short_secret(c).as_bytes(), SCRYPT_SALT.as_bytes(), &params, &mut seed).expect("32 bytes is a valid scrypt length");
    Identity::from_seed(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_codes_look_the_way_the_terminal_prints_them() {
        for _ in 0..200 {
            let c = new_short_code().unwrap();
            assert!(c.id.len() == 8 && c.id.bytes().all(|b| CROCKFORD.contains(&b)), "{}", c.id);
            assert!(c.code.len() == 6 && c.code.bytes().all(|b| b.is_ascii_digit()), "{}", c.code);
            assert_eq!(parse_short_code(&format!("{} {}", format_short_id(&c.id), c.code)), Some(c));
        }
    }
}
