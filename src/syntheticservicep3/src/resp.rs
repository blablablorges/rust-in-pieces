//! RESP2 SET/GET over `tcp::Client`. Only bulk-string and simple-string/error replies
//! are needed; arrays are an error.
use super::tcp;

pub fn resp_encode(args: &[&[u8]]) -> Vec<u8> {
    let mut buf = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        buf.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
        buf.extend_from_slice(arg);
        buf.extend_from_slice(b"\r\n");
    }
    buf
}

fn crlf(b: &[u8]) -> Option<usize> {
    b.windows(2).position(|w| w == b"\r\n")
}

/// Length of the first complete reply in `buf`, if one is there.
pub fn resp_len(buf: &[u8]) -> Result<Option<usize>, String> {
    match buf.first() {
        None => Ok(None),
        Some(b'$') => match crlf(buf) {
            None => Ok(None),
            Some(pos) => {
                let len: i64 = std::str::from_utf8(&buf[1..pos])
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| "redis: bad bulk length".to_string())?;
                if len < 0 {
                    return Ok(Some(pos + 2));
                }
                let total = pos + 2 + len as usize + 2;
                Ok(if buf.len() >= total { Some(total) } else { None })
            }
        },
        Some(b'+') | Some(b'-') | Some(b':') => Ok(crlf(buf).map(|pos| pos + 2)),
        Some(other) => Err(format!("redis: unexpected reply type {other:#04x}")),
    }
}

pub struct Client(tcp::Client);

impl Client {
    /// `redis://host:port/` or `host:port`.
    pub fn new(addr: &str) -> Result<Client, String> {
        let addr = addr.strip_prefix("redis://").unwrap_or(addr).trim_end_matches('/');
        Ok(Client(tcp::Client::new(addr)?))
    }

    pub async fn set(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let reply = self.0.roundtrip(&resp_encode(&[b"SET", key.as_bytes(), value]), resp_len).await?;
        check(&reply).map(|_| ())
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let reply = self.0.roundtrip(&resp_encode(&[b"GET", key.as_bytes()]), resp_len).await?;
        check(&reply)?;
        if reply.starts_with(b"$-1") {
            return Ok(None);
        }
        let pos = crlf(&reply).ok_or("redis: truncated bulk")?;
        let len: usize = std::str::from_utf8(&reply[1..pos])
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or("redis: bad bulk length")?;
        Ok(Some(reply[pos + 2..pos + 2 + len].to_vec()))
    }
}

fn check(reply: &[u8]) -> Result<&[u8], String> {
    if reply.first() == Some(&b'-') {
        return Err(format!("redis: {}", String::from_utf8_lossy(&reply[1..]).trim()));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lengths() {
        assert_eq!(resp_len(b"+OK\r\n").unwrap(), Some(5));
        assert_eq!(resp_len(b"$-1\r\n").unwrap(), Some(5));
        assert_eq!(resp_len(b"$3\r\nabc\r\n").unwrap(), Some(9));
        assert_eq!(resp_len(b"$3\r\nab").unwrap(), None);
        assert_eq!(resp_encode(&[b"GET", b"k"]), b"*2\r\n$3\r\nGET\r\n$1\r\nk\r\n");
    }
}
