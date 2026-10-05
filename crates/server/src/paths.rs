//! `file:` URIs and the paths they name.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let text = uri.as_str();
    let rest = text.strip_prefix("file://")?;
    // `file://host/path` has a host; the empty host is the usual case.
    let path = rest.find('/').map_or("", |index| &rest[index..]);
    let decoded = percent_decode(path)?;
    Some(PathBuf::from(decoded))
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let text = path.to_str()?;
    let mut out = String::from("file://");
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b'$'
            | b'&'
            | b'+'
            | b','
            | b';'
            | b'='
            | b'@'
            | b'!'
            | b'('
            | b')'
            | b'\''
            | b'*' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    Uri::from_str(&out).ok()
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_uris_and_paths() {
        let uri = Uri::from_str("file:///Users/me/My%20Project/a%C3%A9.php").expect("a uri");
        let path = uri_to_path(&uri).expect("a path");
        assert_eq!(path, PathBuf::from("/Users/me/My Project/aé.php"));
        assert_eq!(path_to_uri(&path).expect("a uri"), uri);
        assert_eq!(uri_to_path(&Uri::from_str("untitled:Untitled-1").expect("a uri")), None);
    }
}
