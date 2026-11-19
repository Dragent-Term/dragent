use std::io::{self, BufRead, Write};

const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

pub fn write_message(mut writer: impl Write, value: &serde_json::Value) -> io::Result<()> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message exceeds 16 MiB",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

pub fn read_message(mut reader: impl BufRead) -> io::Result<Option<serde_json::Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let length = content_length
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing LSP Content-Length"))?;
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LSP message exceeds 16 MiB",
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn round_trips_lsp_frame() {
        let value = serde_json::json!({"jsonrpc":"2.0","id":7,"method":"initialize"});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &value).unwrap();
        let decoded = read_message(BufReader::new(Cursor::new(bytes)))
            .unwrap()
            .unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn rejects_missing_content_length() {
        let error =
            read_message(BufReader::new(Cursor::new(b"X-Test: yes\r\n\r\n{}"))).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
