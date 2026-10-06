// src/dht/messages.rs
//! DHT Binary Message Format
//! ==========================
//!
//! Binary serialization for DHT queries and responses

use crate::util::HashId;

/// DHT query type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhtQueryType {
    FindNode = 1,
    Store = 2,
    FindValue = 3,
}

impl DhtQueryType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(DhtQueryType::FindNode),
            2 => Some(DhtQueryType::Store),
            3 => Some(DhtQueryType::FindValue),
            _ => None,
        }
    }

    pub fn to_byte(self) -> u8 {
        match self {
            DhtQueryType::FindNode => 1,
            DhtQueryType::Store => 2,
            DhtQueryType::FindValue => 3,
        }
    }
}
/// DHT query (local representation)
#[derive(Debug, Clone)]
pub struct DhtQuery {
    pub request_id: u64,
    pub query_type: DhtQueryType,
    pub key: HashId,
    pub value: Option<Vec<u8>>,
    pub limit: u8,
}
impl DhtQuery {
    /// Binary serialization:
    /// [0]      = query_type
    /// [1]      = limit
    /// [2..34]  = key (32 bytes)
    /// [34..]   = value (if present; only for Store)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.query_type.to_byte());
        out.push(self.limit);
        out.extend_from_slice(self.key.as_ref());

        if let Some(v) = &self.value {
            out.extend_from_slice(v);
        }

        out
    }

    /// Deserialize from binary
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 34 {
            return None;
        }
        let qt = DhtQueryType::from_byte(buf[0])?;
        let limit = buf[1];

        let mut key = [0u8; 32];
        key.copy_from_slice(&buf[2..34]);

        let value = if qt == DhtQueryType::Store && buf.len() > 34 {
            Some(buf[34..].to_vec())
        } else {
            None
        };

        Some(Self {
            request_id: 0, // Will be set by transport layer
            query_type: qt,
            key: HashId(key),
            value,
            limit,
        })
    }
}

/// DHT response from node
#[derive(Debug, Clone)]
pub struct DhtResponse {
    pub value: Option<Vec<u8>>,
    pub nodes: Vec<(HashId, String)>,
}

impl DhtResponse {
    pub fn new(value: Option<Vec<u8>>, nodes: Vec<(HashId, String)>) -> Self {
        Self { value, nodes }
    }

    /// Binary serialization:
    /// [0]        = has_value (0/1)
    /// [1..3]?    = value_len (u16 BE), if has_value == 1
    /// [..]       = value bytes (value_len), if present
    /// then 0 or more blocks:
    ///   [0..32]  = node_id (HashId)
    ///   [32]     = addr_len (u8)
    ///   [33..]   = addr bytes (UTF-8)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();

        match &self.value {
            Some(v) => {
                out.push(1);
                let len = v.len() as u16;
                out.extend_from_slice(&len.to_be_bytes());
                out.extend_from_slice(v);
            }
            None => {
                out.push(0);
            }
        }

        for (id, addr) in &self.nodes {
            out.extend_from_slice(id.as_ref());
            let bytes = addr.as_bytes();
            out.push(bytes.len() as u8);
            out.extend_from_slice(bytes);
        }

        out
    }

    /// Deserialize from binary
    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }

        let mut idx = 0;
        let has_value = buf[idx];
        idx += 1;

        let mut value = None;

        if has_value != 0 {
            if buf.len() < idx + 2 {
                return None;
            }
            let len = u16::from_be_bytes([buf[idx], buf[idx + 1]]) as usize;
            idx += 2;

            if buf.len() < idx + len {
                return None;
            }

            value = Some(buf[idx..idx + len].to_vec());
            idx += len;
        }

        let mut nodes = Vec::new();

        while idx < buf.len() {
            if buf.len() < idx + 32 + 1 {
                break;
            }

            let mut id = [0u8; 32];
            id.copy_from_slice(&buf[idx..idx + 32]);
            idx += 32;

            let addr_len = buf[idx] as usize;
            idx += 1;

            if buf.len() < idx + addr_len {
                break;
            }

            if let Ok(s) = std::str::from_utf8(&buf[idx..idx + addr_len]) {
                nodes.push((HashId(id), s.to_string()));
            }

            idx += addr_len;
        }

        Some(Self { value, nodes })
    }
}
