//! A multipart/form-data body read into its parts, as `Response.formData()` gives them: a part with
//! a filename is a file, any other is a string field. Uploads are the hub's only multipart route,
//! and their body is read whole (and counted against the upload limit) before it gets here.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    File { name: String, mime: String, data: Vec<u8> },
}

#[derive(Debug, Default)]
pub struct Form(pub Vec<(String, Part)>);

impl Form {
    /// `form.get(name)`: the first part of that name
    pub fn get(&self, name: &str) -> Option<&Part> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }

    /// `form.get(name)` when it is a string field
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.get(name) {
            Some(Part::Text(s)) => Some(s),
            _ => None,
        }
    }

    /// `form.getAll(name)`
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Part> + 'a {
        self.0.iter().filter(move |(n, _)| n == name).map(|(_, p)| p)
    }
}

/// The value of one parameter in a header like `form-data; name="files"; filename="a.png"`.
fn param(header: &str, key: &str) -> Option<String> {
    let mut rest = header;
    while let Some(i) = rest.find(';') {
        rest = rest[i + 1..].trim_start();
        let Some(eq) = rest.find('=') else { break };
        let k = rest[..eq].trim();
        let after = &rest[eq + 1..];
        let (value, next) = if let Some(quoted) = after.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((j, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, n)) = chars.next() {
                            out.push(n);
                        }
                    }
                    '"' => {
                        end = j + 1;
                        break;
                    }
                    c => out.push(c),
                }
            }
            (out, &quoted[end.min(quoted.len())..])
        } else {
            let end = after.find(';').unwrap_or(after.len());
            (after[..end].trim().to_owned(), &after[end..])
        };
        if k.eq_ignore_ascii_case(key) {
            return Some(value);
        }
        rest = next;
    }
    None
}

/// The boundary a Content-Type names, if it is multipart/form-data.
pub fn boundary(content_type: &str) -> Option<String> {
    let kind = content_type.split(';').next().unwrap_or("").trim();
    if !kind.eq_ignore_ascii_case("multipart/form-data") {
        return None;
    }
    param(content_type, "boundary").filter(|b| !b.is_empty() && b.len() <= 200)
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Splits a body into its parts; None when it isn't well-formed multipart.
pub fn parse(content_type: &str, body: &[u8]) -> Option<Form> {
    let b = boundary(content_type)?;
    let delim = format!("--{b}").into_bytes();
    let mut at = find(body, &delim, 0)?;
    let mut form = Form::default();
    loop {
        at += delim.len();
        // the closing delimiter
        if body.get(at..at + 2) == Some(b"--") {
            return Some(form);
        }
        // transport padding, then the line break before the part's headers
        while matches!(body.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        if body.get(at..at + 2) != Some(b"\r\n") {
            return None;
        }
        at += 2;
        let head_end = find(body, b"\r\n\r\n", at)?;
        let head = String::from_utf8_lossy(&body[at..head_end]).into_owned();
        let data_at = head_end + 4;
        let mut next_delim = Vec::with_capacity(delim.len() + 2);
        next_delim.extend_from_slice(b"\r\n");
        next_delim.extend_from_slice(&delim);
        let end = find(body, &next_delim, data_at)?;
        let data = &body[data_at..end];
        let (mut disposition, mut mime) = (String::new(), String::new());
        for line in head.split("\r\n") {
            let Some((k, v)) = line.split_once(':') else { continue };
            if k.trim().eq_ignore_ascii_case("content-disposition") {
                disposition = v.trim().to_owned();
            } else if k.trim().eq_ignore_ascii_case("content-type") {
                mime = v.trim().to_owned();
            }
        }
        if !disposition.to_ascii_lowercase().starts_with("form-data") {
            return None;
        }
        let name = param(&disposition, "name")?;
        let part = match param(&disposition, "filename") {
            Some(filename) => Part::File { name: filename, mime: mime.to_ascii_lowercase(), data: data.to_vec() },
            None => Part::Text(String::from_utf8_lossy(data).into_owned()),
        };
        form.0.push((name, part));
        at = end + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_fields_come_apart() {
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"pane\"\r\n\r\n12\r\n--XyZ\r\nContent-Disposition: form-data; name=\"files\"; filename=\"a \\\"b\\\".png\"\r\nContent-Type: image/PNG\r\n\r\n\x89PNG\r\n\r\n--XyZ\r\nContent-Disposition: form-data; name=\"files\"; filename=\"c.txt\"\r\n\r\nhi\r\n--XyZ--\r\n";
        let f = parse("multipart/form-data; boundary=XyZ", body).unwrap();
        assert_eq!(f.text("pane"), Some("12"));
        let files: Vec<&Part> = f.all("files").collect();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], &Part::File { name: "a \"b\".png".into(), mime: "image/png".into(), data: b"\x89PNG\r\n".to_vec() });
        assert_eq!(files[1], &Part::File { name: "c.txt".into(), mime: String::new(), data: b"hi".to_vec() });
    }

    #[test]
    fn anything_else_is_refused() {
        assert!(parse("application/json", b"{}").is_none());
        assert!(parse("multipart/form-data; boundary=a", b"nothing here").is_none());
        assert!(parse("multipart/form-data; boundary=a", b"--a\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nno end").is_none());
        assert_eq!(boundary("multipart/form-data; boundary=\"q r\""), Some("q r".into()));
    }
}
