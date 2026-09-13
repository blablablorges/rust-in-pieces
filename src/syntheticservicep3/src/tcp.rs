//! One TCP connection on wasi:sockets 0.3, held for the life of the instance behind an
//! async mutex and re-dialed after a failed exchange: the lifecycle the P2 and native
//! ports give their datastore sockets (`Mutex<Option<TcpStream>>`).
use futures::lock::Mutex;
use wasip3::sockets::ip_name_lookup::resolve_addresses;
use wasip3::sockets::types::{
    ErrorCode, IpAddress, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress,
    TcpSocket,
};
use wasip3::wit_bindgen::rt::async_support::{FutureReader, StreamReader, StreamWriter};
use wasip3::wit_stream;

pub struct Conn {
    _sock: TcpSocket,
    tx: StreamWriter<u8>,
    rx: StreamReader<u8>,
    // Dropping these would cancel the host-side copy tasks; they are never awaited.
    _sending: FutureReader<Result<(), ErrorCode>>,
    _receiving: FutureReader<Result<(), ErrorCode>>,
}

pub struct Client {
    host: String,
    port: u16,
    conn: Mutex<Option<Conn>>,
}

impl Client {
    /// `host:port`; the host is resolved at first dial, once per connection.
    pub fn new(addr: &str) -> Result<Client, String> {
        let (host, port) = addr
            .rsplit_once(':')
            .ok_or_else(|| format!("address {addr} has no port"))?;
        let port = port.parse().map_err(|_| format!("bad port in {addr}"))?;
        Ok(Client { host: host.to_string(), port, conn: Mutex::new(None) })
    }

    /// Send `payload`, read until `done` reports a whole reply, return that reply.
    /// A failure on the held connection drops it and dials once more.
    pub async fn roundtrip<F>(&self, payload: &[u8], done: F) -> Result<Vec<u8>, String>
    where
        F: Fn(&[u8]) -> Result<Option<usize>, String>,
    {
        let mut guard = self.conn.lock().await;
        if let Some(c) = guard.as_mut() {
            match exchange(c, payload, &done).await {
                Ok(reply) => return Ok(reply),
                Err(_) => *guard = None,
            }
        }
        let mut c = self.dial().await?;
        let reply = exchange(&mut c, payload, &done).await?;
        *guard = Some(c);
        Ok(reply)
    }

    async fn dial(&self) -> Result<Conn, String> {
        let ip = match self.host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(v)) => {
                let o = v.octets();
                IpAddress::Ipv4((o[0], o[1], o[2], o[3]))
            }
            Ok(std::net::IpAddr::V6(v)) => {
                let s = v.segments();
                IpAddress::Ipv6((s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]))
            }
            Err(_) => resolve_addresses(self.host.clone())
                .await
                .map_err(|e| format!("resolve {}: {e:?}", self.host))?
                .into_iter()
                .next()
                .ok_or_else(|| format!("no address for {}", self.host))?,
        };
        let (family, remote) = match ip {
            IpAddress::Ipv4(address) => (
                IpAddressFamily::Ipv4,
                IpSocketAddress::Ipv4(Ipv4SocketAddress { port: self.port, address }),
            ),
            IpAddress::Ipv6(address) => (
                IpAddressFamily::Ipv6,
                IpSocketAddress::Ipv6(Ipv6SocketAddress {
                    port: self.port,
                    flow_info: 0,
                    address,
                    scope_id: 0,
                }),
            ),
        };
        let sock = TcpSocket::create(family).map_err(|e| format!("socket: {e:?}"))?;
        sock.connect(remote)
            .await
            .map_err(|e| format!("connect {}:{}: {e:?}", self.host, self.port))?;
        let (tx, tx_reader) = wit_stream::new::<u8>();
        let sending = sock.send(tx_reader);
        let (rx, receiving) = sock.receive();
        Ok(Conn { _sock: sock, tx, rx, _sending: sending, _receiving: receiving })
    }
}

async fn exchange<F>(c: &mut Conn, payload: &[u8], done: &F) -> Result<Vec<u8>, String>
where
    F: Fn(&[u8]) -> Result<Option<usize>, String>,
{
    let rest = c.tx.write_all(payload.to_vec()).await;
    if !rest.is_empty() {
        return Err("short write".into());
    }
    let mut buf = Vec::new();
    loop {
        if let Some(n) = done(&buf)? {
            buf.truncate(n);
            return Ok(buf);
        }
        let (_status, chunk) = c.rx.read(Vec::with_capacity(8192)).await;
        if chunk.is_empty() {
            return Err("peer closed mid-reply".into());
        }
        buf.extend_from_slice(&chunk);
    }
}
