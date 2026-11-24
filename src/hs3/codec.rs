//! Строгий кодек сообщений рукопожатия версии 3 (docs/HANDSHAKE_HARDENING.md, раздел 3а). Без ввода-вывода и сети.
//!
//! Разбор отвергает: лишние байты в конце, ненулевые зарезервированные флаги, неизвестные версию, тип, набор и контекст, неверную длину,
//! невалидный UTF-8 в строках, несовпадение `node_id` с ключом подписи и неверную подпись. Хэшируются полученные байты, а не пересобранный
//! объект, поэтому у `Request`/`Ack` рядом с полями хранятся их точные байты.
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

pub const MAGIC: [u8; 4] = *b"YHS3";
pub const VERSION: u8 = 3;
pub const SUITE: u8 = 1;
pub const HEADER_LEN: usize = 9;
pub const FIN_LEN: usize = HEADER_LEN + 32 + 32;
pub const KEY_CONFIRM_LEN: usize = HEADER_LEN + 32 + 16;
pub const MAX_JURISDICTION: usize = 16;
pub const MAX_NAME: usize = 64;
/// Наибольшие размеры по схеме (строки максимальной длины): запрос 311, ответ 343.
pub const MAX_REQUEST: usize = 311;
pub const MAX_ACK: usize = 343;
const SIG_LABEL_REQ: &[u8] = b"yandi-hs-v3-req";
const SIG_LABEL_ACK: &[u8] = b"yandi-hs-v3-ack";

pub const CTX_NETLAYER_UDP: u8 = 1;
pub const CTX_NETLAYER_WS: u8 = 2;
pub const CTX_P2P_CHAT: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    Request = 1,
    Ack = 2,
    FinI = 3,
    FinR = 4,
    KeyConfirm = 5,
}

impl MsgType {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Request,
            2 => Self::Ack,
            3 => Self::FinI,
            4 => Self::FinR,
            5 => Self::KeyConfirm,
            _ => return None,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CodecError {
    Short,
    Magic,
    Version,
    Type,
    UnexpectedType,
    Suite,
    Context,
    Flags,
    Trailing,
    Utf8,
    StringTooLong,
    NodeIdMismatch,
    BadSignature,
}

fn header(t: MsgType, context: u8) -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(&MAGIC);
    h[4] = VERSION;
    h[5] = t as u8;
    h[6] = SUITE;
    h[7] = context;
    h[8] = 0;
    h
}

/// Разбор шапки: возвращает (тип, контекст).
pub fn parse_header(b: &[u8]) -> Result<(MsgType, u8), CodecError> {
    if b.len() < HEADER_LEN {
        return Err(CodecError::Short);
    }
    if b[..4] != MAGIC {
        return Err(CodecError::Magic);
    }
    if b[4] != VERSION {
        return Err(CodecError::Version);
    }
    let t = MsgType::from_u8(b[5]).ok_or(CodecError::Type)?;
    if b[6] != SUITE {
        return Err(CodecError::Suite);
    }
    if !matches!(b[7], CTX_NETLAYER_UDP | CTX_NETLAYER_WS | CTX_P2P_CHAT) {
        return Err(CodecError::Context);
    }
    if b[8] != 0 {
        return Err(CodecError::Flags);
    }
    Ok((t, b[7]))
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], CodecError> {
        if self.b.len() - self.i < n {
            return Err(CodecError::Short);
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn arr<const N: usize>(&mut self) -> Result<[u8; N], CodecError> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
    fn string(&mut self, max: usize) -> Result<String, CodecError> {
        let len = self.take(1)?[0] as usize;
        if len > max {
            return Err(CodecError::StringTooLong);
        }
        String::from_utf8(self.take(len)?.to_vec()).map_err(|_| CodecError::Utf8)
    }
}

fn put_string(out: &mut Vec<u8>, s: &str, max: usize) {
    assert!(s.len() <= max, "строка длиннее допустимого — ошибка вызывающего");
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

/// Запрос инициатора.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub context: u8,
    pub node_id: [u8; 32],
    pub signing_pub: [u8; 32],
    pub ea: [u8; 32],
    pub time_ms: u64,
    pub nonce: [u8; 16],
    /// Нули — цель неизвестна (допустимо только в контексте 1, решает вызывающий).
    pub expected_peer: [u8; 32],
    pub capabilities: u32,
    pub jurisdiction: String,
    pub name: String,
}

impl Request {
    /// Канонические байты со своей подписью.
    pub fn encode(&self, sk: &SigningKey) -> Vec<u8> {
        let mut out = header(MsgType::Request, self.context).to_vec();
        out.extend_from_slice(&self.node_id);
        out.extend_from_slice(&self.signing_pub);
        out.extend_from_slice(&self.ea);
        out.extend_from_slice(&self.time_ms.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.expected_peer);
        out.extend_from_slice(&self.capabilities.to_be_bytes());
        put_string(&mut out, &self.jurisdiction, MAX_JURISDICTION);
        put_string(&mut out, &self.name, MAX_NAME);
        let sig = sk.sign(&[SIG_LABEL_REQ, &out].concat());
        out.extend_from_slice(&sig.to_bytes());
        out
    }

    /// Разбор с проверкой строгости, привязки `node_id` и подписи.
    pub fn decode(b: &[u8]) -> Result<Self, CodecError> {
        let (t, context) = parse_header(b)?;
        if t != MsgType::Request {
            return Err(CodecError::UnexpectedType);
        }
        let mut r = Reader { b, i: HEADER_LEN };
        let node_id = r.arr::<32>()?;
        let signing_pub = r.arr::<32>()?;
        let ea = r.arr::<32>()?;
        let time_ms = u64::from_be_bytes(r.arr::<8>()?);
        let nonce = r.arr::<16>()?;
        let expected_peer = r.arr::<32>()?;
        let capabilities = u32::from_be_bytes(r.arr::<4>()?);
        let jurisdiction = r.string(MAX_JURISDICTION)?;
        let name = r.string(MAX_NAME)?;
        let signed_end = r.i;
        let sig = r.arr::<64>()?;
        if r.i != b.len() {
            return Err(CodecError::Trailing);
        }
        verify(SIG_LABEL_REQ, &b[..signed_end], &node_id, &signing_pub, &sig)?;
        Ok(Self { context, node_id, signing_pub, ea, time_ms, nonce, expected_peer, capabilities, jurisdiction, name })
    }
}

/// Ответ ответчика.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ack {
    pub context: u8,
    pub node_id: [u8; 32],
    pub signing_pub: [u8; 32],
    pub eb: [u8; 32],
    pub time_ms: u64,
    pub nonce: [u8; 16],
    /// SHA-256 точных байт запроса.
    pub h_r: [u8; 32],
    pub expected_initiator: [u8; 32],
    pub capabilities: u32,
    pub jurisdiction: String,
    pub name: String,
}

impl Ack {
    pub fn encode(&self, sk: &SigningKey) -> Vec<u8> {
        let mut out = header(MsgType::Ack, self.context).to_vec();
        out.extend_from_slice(&self.node_id);
        out.extend_from_slice(&self.signing_pub);
        out.extend_from_slice(&self.eb);
        out.extend_from_slice(&self.time_ms.to_be_bytes());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.h_r);
        out.extend_from_slice(&self.expected_initiator);
        out.extend_from_slice(&self.capabilities.to_be_bytes());
        put_string(&mut out, &self.jurisdiction, MAX_JURISDICTION);
        put_string(&mut out, &self.name, MAX_NAME);
        let sig = sk.sign(&[SIG_LABEL_ACK, &out].concat());
        out.extend_from_slice(&sig.to_bytes());
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self, CodecError> {
        let (t, context) = parse_header(b)?;
        if t != MsgType::Ack {
            return Err(CodecError::UnexpectedType);
        }
        let mut r = Reader { b, i: HEADER_LEN };
        let node_id = r.arr::<32>()?;
        let signing_pub = r.arr::<32>()?;
        let eb = r.arr::<32>()?;
        let time_ms = u64::from_be_bytes(r.arr::<8>()?);
        let nonce = r.arr::<16>()?;
        let h_r = r.arr::<32>()?;
        let expected_initiator = r.arr::<32>()?;
        let capabilities = u32::from_be_bytes(r.arr::<4>()?);
        let jurisdiction = r.string(MAX_JURISDICTION)?;
        let name = r.string(MAX_NAME)?;
        let signed_end = r.i;
        let sig = r.arr::<64>()?;
        if r.i != b.len() {
            return Err(CodecError::Trailing);
        }
        verify(SIG_LABEL_ACK, &b[..signed_end], &node_id, &signing_pub, &sig)?;
        Ok(Self { context, node_id, signing_pub, eb, time_ms, nonce, h_r, expected_initiator, capabilities, jurisdiction, name })
    }
}

fn verify(label: &[u8], signed: &[u8], node_id: &[u8; 32], signing_pub: &[u8; 32], sig: &[u8; 64]) -> Result<(), CodecError> {
    if &crate::util::types::derive_node_id(signing_pub) != node_id {
        return Err(CodecError::NodeIdMismatch);
    }
    let vk = VerifyingKey::from_bytes(signing_pub).map_err(|_| CodecError::BadSignature)?;
    vk.verify_strict(&[label, signed].concat(), &Signature::from_bytes(sig)).map_err(|_| CodecError::BadSignature)
}

/// Подтверждение ключа (`FinI` или `FinR`): шапка, `T` и MAC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fin {
    pub kind: MsgType,
    pub context: u8,
    pub t: [u8; 32],
    pub mac: [u8; 32],
}

impl Fin {
    pub fn encode(&self) -> Vec<u8> {
        assert!(matches!(self.kind, MsgType::FinI | MsgType::FinR));
        let mut out = header(self.kind, self.context).to_vec();
        out.extend_from_slice(&self.t);
        out.extend_from_slice(&self.mac);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self, CodecError> {
        let (kind, context) = parse_header(b)?;
        if !matches!(kind, MsgType::FinI | MsgType::FinR) {
            return Err(CodecError::UnexpectedType);
        }
        if b.len() < FIN_LEN {
            return Err(CodecError::Short);
        }
        if b.len() > FIN_LEN {
            return Err(CodecError::Trailing);
        }
        let mut r = Reader { b, i: HEADER_LEN };
        Ok(Self { kind, context, t: r.arr::<32>()?, mac: r.arr::<32>()? })
    }
}

/// Кадр `KeyConfirm`: шапка, `T` и тег AEAD пустого текста (сам AEAD выполняет сеанс).
pub fn key_confirm_frame(context: u8, t: &[u8; 32], tag: &[u8; 16]) -> Vec<u8> {
    let mut out = header(MsgType::KeyConfirm, context).to_vec();
    out.extend_from_slice(t);
    out.extend_from_slice(tag);
    out
}

/// Разбор кадра `KeyConfirm`: (контекст, `T`, тег). AAD для AEAD — первые `HEADER_LEN + 32` байт.
pub fn parse_key_confirm(b: &[u8]) -> Result<(u8, [u8; 32], [u8; 16]), CodecError> {
    let (t, context) = parse_header(b)?;
    if t != MsgType::KeyConfirm {
        return Err(CodecError::UnexpectedType);
    }
    if b.len() < KEY_CONFIRM_LEN {
        return Err(CodecError::Short);
    }
    if b.len() > KEY_CONFIRM_LEN {
        return Err(CodecError::Trailing);
    }
    let mut r = Reader { b, i: HEADER_LEN };
    Ok((context, r.arr::<32>()?, r.arr::<16>()?))
}
