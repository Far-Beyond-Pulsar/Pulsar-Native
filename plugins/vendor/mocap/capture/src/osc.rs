//! A small OSC 1.0 parser: what VMC needs (messages, bundles, `f i s` and the
//! zero-size tags), and nothing that allocates from an untrusted length.

/// One OSC argument.
#[derive(Clone, Debug, PartialEq)]
pub enum OscArg {
    Float(f32),
    Int(i32),
    Str(String),
}

/// One OSC message.
#[derive(Clone, Debug, PartialEq)]
pub struct OscMessage {
    pub address: String,
    pub args: Vec<OscArg>,
}

impl OscMessage {
    /// Argument `i` as a float (an int converts).
    pub fn float(&self, i: usize) -> Option<f32> {
        match self.args.get(i)? {
            OscArg::Float(f) => Some(*f),
            OscArg::Int(n) => Some(*n as f32),
            OscArg::Str(_) => None,
        }
    }

    /// Argument `i` as a string.
    pub fn string(&self, i: usize) -> Option<&str> {
        match self.args.get(i)? {
            OscArg::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let bytes = self.buf.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(bytes)
    }

    /// A NUL-terminated string padded to four bytes.
    fn string(&mut self) -> Option<String> {
        let rest = self.buf.get(self.pos..)?;
        let end = rest.iter().position(|b| *b == 0)?;
        let text = std::str::from_utf8(&rest[..end]).ok()?.to_owned();
        self.pos += (end + 4) & !3;
        (self.pos <= self.buf.len()).then_some(text)
    }

    fn be32(&mut self) -> Option<[u8; 4]> {
        self.take(4)?.try_into().ok()
    }
}

/// Parses a datagram (a message or a bundle) into its messages; malformed
/// content ends parsing of that element and keeps what came before.
pub fn parse_packet(buf: &[u8]) -> Vec<OscMessage> {
    let mut out = Vec::new();
    parse_into(buf, &mut out, 0);
    out
}

fn parse_into(buf: &[u8], out: &mut Vec<OscMessage>, depth: u8) {
    if buf.starts_with(b"#bundle\0") {
        if depth < 4 {
            parse_bundle(buf, out, depth);
        }
    } else if let Some(message) = parse_message(buf) {
        out.push(message);
    }
}

fn parse_bundle(buf: &[u8], out: &mut Vec<OscMessage>, depth: u8) {
    let mut cursor = Cursor { buf, pos: 16 };
    while let Some(size) = cursor.be32() {
        let Some(element) = cursor.take(u32::from_be_bytes(size) as usize) else {
            return;
        };
        parse_into(element, out, depth + 1);
    }
}

fn parse_message(buf: &[u8]) -> Option<OscMessage> {
    let mut cursor = Cursor { buf, pos: 0 };
    let address = cursor.string()?;
    if !address.starts_with('/') {
        return None;
    }
    let tags = cursor.string()?;
    let mut args = Vec::new();
    for tag in tags.strip_prefix(',')?.chars() {
        match tag {
            'f' => args.push(OscArg::Float(f32::from_be_bytes(cursor.be32()?))),
            'i' => args.push(OscArg::Int(i32::from_be_bytes(cursor.be32()?))),
            's' => args.push(OscArg::Str(cursor.string()?)),
            'T' | 'F' | 'N' | 'I' => {}
            'h' | 'd' | 't' => {
                cursor.take(8)?;
            }
            _ => return Some(OscMessage { address, args }),
        }
    }
    Some(OscMessage { address, args })
}

/// Encodes a message (used by tests and the synthetic VMC sender).
pub fn encode_message(address: &str, args: &[OscArg]) -> Vec<u8> {
    fn pad(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(text.as_bytes());
        out.push(0);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    let mut out = Vec::new();
    pad(&mut out, address);
    let tags: String = std::iter::once(',')
        .chain(args.iter().map(|a| match a {
            OscArg::Float(_) => 'f',
            OscArg::Int(_) => 'i',
            OscArg::Str(_) => 's',
        }))
        .collect();
    pad(&mut out, &tags);
    for arg in args {
        match arg {
            OscArg::Float(f) => out.extend_from_slice(&f.to_be_bytes()),
            OscArg::Int(n) => out.extend_from_slice(&n.to_be_bytes()),
            OscArg::Str(s) => pad(&mut out, s),
        }
    }
    out
}

/// Wraps encoded messages in a bundle.
pub fn encode_bundle(messages: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"#bundle\0".to_vec();
    out.extend_from_slice(&1u64.to_be_bytes());
    for message in messages {
        out.extend_from_slice(&(message.len() as u32).to_be_bytes());
        out.extend_from_slice(message);
    }
    out
}
