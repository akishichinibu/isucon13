use std::collections::HashSet;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use parking_lot::RwLock;
use socket2::{Domain, Protocol, Socket, Type};

const DNS_PORT: u16 = 53;
const WORKERS: usize = 4;
const TTL: u32 = 0;

static A_RR: AtomicUsize = AtomicUsize::new(0);

/// In-process authoritative UDP/53 for u.isucon.dev.
/// Answers list every webapp TLS IP. Order rotates so clients that pick the
/// first A split across the nginx edges.
pub fn start(names: Arc<RwLock<HashSet<String>>>, ipv4s: Vec<[u8; 4]>) {
    assert!(!ipv4s.is_empty(), "at least one A-record IP");
    let ipv4s = Arc::new(ipv4s);
    for i in 0..WORKERS {
        let names = names.clone();
        let ipv4s = ipv4s.clone();
        thread::Builder::new()
            .name(format!("dns-{i}"))
            .spawn(move || {
                if let Err(err) = worker(names, ipv4s) {
                    tracing::error!("dns worker failed: {err}");
                }
            })
            .expect("spawn dns worker");
    }
    tracing::info!("in-process DNS listening on UDP/{DNS_PORT}");
}

fn worker(names: Arc<RwLock<HashSet<String>>>, ipv4s: Arc<Vec<[u8; 4]>>) -> std::io::Result<()> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    let _ = sock.set_reuse_port(true);
    let _ = sock.set_recv_buffer_size(4 * 1024 * 1024);
    let _ = sock.set_send_buffer_size(4 * 1024 * 1024);
    sock.bind(
        &format!("0.0.0.0:{DNS_PORT}")
            .parse::<SocketAddr>()
            .unwrap()
            .into(),
    )?;
    let udp: UdpSocket = sock.into();
    let mut buf = [0u8; 2048];
    loop {
        let (n, addr) = match udp.recv_from(&mut buf) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!("dns recv: {err}");
                continue;
            }
        };
        if let Some(resp) = handle_query(&buf[..n], &names, &ipv4s) {
            let _ = udp.send_to(&resp, addr);
        }
    }
}

fn encode_a(out: &mut Vec<u8>, ipv4: [u8; 4]) {
    out.extend_from_slice(&[0xc0, 0x0c]); // pointer to QNAME
    out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
    out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    out.extend_from_slice(&TTL.to_be_bytes());
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(&ipv4);
}

fn encode_a_records(ipv4s: &[[u8; 4]]) -> Vec<u8> {
    let n = ipv4s.len();
    let start = A_RR.fetch_add(1, Ordering::Relaxed) % n;
    let mut rdata = Vec::with_capacity(n * 16);
    for i in 0..n {
        encode_a(&mut rdata, ipv4s[(start + i) % n]);
    }
    rdata
}

fn handle_query(msg: &[u8], names: &RwLock<HashSet<String>>, ipv4s: &[[u8; 4]]) -> Option<Vec<u8>> {
    if msg.len() < 12 {
        return None;
    }
    let qdcount = u16::from_be_bytes([msg[4], msg[5]]);
    if qdcount == 0 {
        return None;
    }
    let (qname, qtype, qclass, qend) = decode_question(msg)?;
    if qclass != 1 {
        return Some(build_response(msg, qend, 0, 0, &[])); // NOTIMP-ish: NOERROR empty
    }

    let exists = {
        let set = names.read();
        set.contains(&qname)
    };

    match qtype {
        1 if exists => {
            let rdata = encode_a_records(ipv4s);
            Some(build_response(msg, qend, 0, ipv4s.len() as u16, &rdata))
        }
        2 if qname == crate::store::ZONE => {
            // NS for apex
            let mut rdata = Vec::new();
            rdata.extend_from_slice(&[0xc0, 0x0c]);
            rdata.extend_from_slice(&2u16.to_be_bytes());
            rdata.extend_from_slice(&1u16.to_be_bytes());
            rdata.extend_from_slice(&TTL.to_be_bytes());
            let mut ns = Vec::new();
            encode_name(&mut ns, "ns1.u.isucon.dev");
            rdata.extend_from_slice(&(ns.len() as u16).to_be_bytes());
            rdata.extend(ns);
            Some(build_response(msg, qend, 0, 1, &rdata))
        }
        6 if qname == crate::store::ZONE => {
            let soa = encode_soa_rr();
            Some(build_response(msg, qend, 0, 1, &soa))
        }
        _ if exists => {
            let soa = encode_soa_rr();
            Some(build_nodata(msg, qend, &soa))
        }
        _ => {
            let soa = encode_soa_rr();
            Some(build_nxdomain(msg, qend, &soa))
        }
    }
}

fn decode_question(msg: &[u8]) -> Option<(String, u16, u16, usize)> {
    let (name, pos) = decode_name(msg, 12)?;
    if pos + 4 > msg.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
    let qclass = u16::from_be_bytes([msg[pos + 2], msg[pos + 3]]);
    Some((name, qtype, qclass, pos + 4))
}

fn decode_name(msg: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut end_pos = pos;
    let mut hops = 0u8;
    loop {
        if pos >= msg.len() {
            return None;
        }
        let len = msg[pos];
        if len == 0 {
            pos += 1;
            if !jumped {
                end_pos = pos;
            }
            break;
        }
        if len & 0xc0 == 0xc0 {
            if pos + 1 >= msg.len() {
                return None;
            }
            let off = (((len as usize) & 0x3f) << 8) | msg[pos + 1] as usize;
            if !jumped {
                end_pos = pos + 2;
            }
            jumped = true;
            pos = off;
            hops += 1;
            if hops > 10 {
                return None;
            }
            continue;
        }
        if len & 0xc0 != 0 {
            return None;
        }
        pos += 1;
        let end = pos + len as usize;
        if end > msg.len() {
            return None;
        }
        labels.push(String::from_utf8_lossy(&msg[pos..end]).to_ascii_lowercase());
        pos = end;
        if !jumped {
            end_pos = pos;
        }
    }
    Some((labels.join("."), end_pos))
}

fn encode_name(out: &mut Vec<u8>, name: &str) {
    if name.is_empty() {
        out.push(0);
        return;
    }
    for label in name.split('.') {
        let bytes = label.as_bytes();
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
    }
    out.push(0);
}

fn encode_soa_rr() -> Vec<u8> {
    let mut rr = Vec::new();
    encode_name(&mut rr, crate::store::ZONE);
    rr.extend_from_slice(&6u16.to_be_bytes());
    rr.extend_from_slice(&1u16.to_be_bytes());
    rr.extend_from_slice(&TTL.to_be_bytes());
    let mut rdata = Vec::new();
    encode_name(&mut rdata, "ns1.u.isucon.dev");
    encode_name(&mut rdata, "hostmaster.u.isucon.dev");
    rdata.extend_from_slice(&0u32.to_be_bytes());
    rdata.extend_from_slice(&10800u32.to_be_bytes());
    rdata.extend_from_slice(&3600u32.to_be_bytes());
    rdata.extend_from_slice(&604800u32.to_be_bytes());
    rdata.extend_from_slice(&3600u32.to_be_bytes());
    rr.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    rr.extend(rdata);
    rr
}

fn copy_header_question(msg: &[u8], qend: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);
    out.extend_from_slice(&msg[..qend.min(msg.len())]);
    if out.len() < 12 {
        out.resize(12, 0);
    }
    out
}

fn set_flags(out: &mut [u8], msg: &[u8], rcode: u16, ancount: u16, nscount: u16) {
    let flags_in = u16::from_be_bytes([msg[2], msg[3]]);
    let rd = flags_in & 0x0100;
    let opcode = flags_in & 0x7800;
    let flags = 0x8000 | 0x0400 | rd | opcode | rcode; // QR | AA | RD | OPCODE | RCODE
    out[2..4].copy_from_slice(&flags.to_be_bytes());
    out[4..6].copy_from_slice(&1u16.to_be_bytes());
    out[6..8].copy_from_slice(&ancount.to_be_bytes());
    out[8..10].copy_from_slice(&nscount.to_be_bytes());
    out[10..12].copy_from_slice(&0u16.to_be_bytes());
}

fn build_response(msg: &[u8], qend: usize, rcode: u16, ancount: u16, records: &[u8]) -> Vec<u8> {
    let mut out = copy_header_question(msg, qend);
    set_flags(&mut out, msg, rcode, ancount, 0);
    out.extend_from_slice(records);
    out
}

fn build_nxdomain(msg: &[u8], qend: usize, soa: &[u8]) -> Vec<u8> {
    let mut out = copy_header_question(msg, qend);
    set_flags(&mut out, msg, 3, 0, 1);
    out.extend_from_slice(soa);
    out
}

fn build_nodata(msg: &[u8], qend: usize, soa: &[u8]) -> Vec<u8> {
    let mut out = copy_header_question(msg, qend);
    set_flags(&mut out, msg, 0, 0, 1);
    out.extend_from_slice(soa);
    out
}
