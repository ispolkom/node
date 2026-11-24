//! The only tunnel block wire format: typed, versioned, length-delimited v1.
use std::sync::atomic::Ordering;

pub(crate) const SOCKS5: u8 = 0xC8;
pub(crate) const HTTP_CONNECT: u8 = 0xC9;
const VERSION: u8 = 1;
const HEADER: usize = 23;

pub(crate) fn is_tunnel(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(&SOCKS5) | Some(&HTTP_CONNECT))
}

pub(crate) fn reject_invalid(bytes: &[u8]) -> bool {
    is_tunnel(bytes) && validate(bytes).is_err()
}

/// Validate before allocating payload memory or touching any tunnel state.
/// A recognized tunnel type must never fall through to another dispatcher type.
pub(crate) fn validate(bytes: &[u8]) -> Result<(), &'static str> {
    let result = if !is_tunnel(bytes) {
        Err("not a tunnel block")
    } else if bytes.len() < 2 {
        Err("missing tunnel version")
    } else if bytes[1] != VERSION {
        super::transport::get_wagon_stats()
            .tunnel_version_rejected
            .fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            version = bytes[1],
            "[rx] unsupported tunnel block version; dropping block"
        );
        return Err("unsupported tunnel version");
    } else if bytes.len() < HEADER {
        Err("truncated tunnel header")
    } else {
        let len = u32::from_be_bytes(bytes[19..23].try_into().unwrap()) as usize;
        let offset = u64::from_be_bytes(bytes[10..18].try_into().unwrap());
        if bytes.len().checked_sub(HEADER) != Some(len) {
            Err("tunnel payload length mismatch")
        } else if bytes[18] > 2
            || (bytes[18] == 1 && len != 0)
            || (bytes[18] == 2 && len != 4)
            || offset.checked_add(len as u64).is_none()
        {
            Err("invalid tunnel block")
        } else {
            Ok(())
        }
    };
    if let Err(reason) = result {
        if is_tunnel(bytes) {
            super::transport::get_wagon_stats()
                .tunnel_stream_invalid
                .fetch_add(1, Ordering::Relaxed);
            tracing::warn!(reason, "[rx] invalid tunnel block; dropping block");
        }
    }
    result
}

pub(crate) fn encode(kind: u8, id: u64, offset: u64, close: bool, data: &[u8]) -> Vec<u8> {
    let len = u32::try_from(data.len()).expect("tunnel block exceeds u32 length");
    let mut bytes = Vec::with_capacity(HEADER + data.len());
    bytes.extend_from_slice(&[kind, VERSION]);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&offset.to_be_bytes());
    bytes.push(close as u8);
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(data);
    bytes
}

pub(crate) fn decode(bytes: &[u8], kind: u8) -> Option<(u64, u64, bool, &[u8])> {
    if bytes.first() != Some(&kind) {
        return None;
    }
    validate(bytes).ok()?;
    if bytes[18] == 2 {
        return None; // служебный блок окна, не данные потока
    }
    Some((
        u64::from_be_bytes(bytes[2..10].try_into().ok()?),
        u64::from_be_bytes(bytes[10..18].try_into().ok()?),
        bytes[18] == 1,
        &bytes[HEADER..],
    ))
}

/// Окно получателя: «в TCP выдано `consumed` байт моего входящего потока, готов принять ещё `window`».
/// Это регулятор скорости отправителя, а не подтверждение доставки блоков и не повтор: значения монотонны, потеря сообщения
/// восстанавливается следующим.
pub(crate) fn encode_window(kind: u8, id: u64, consumed: u64, window: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER + 4);
    bytes.extend_from_slice(&[kind, VERSION]);
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&consumed.to_be_bytes());
    bytes.push(2);
    bytes.extend_from_slice(&4u32.to_be_bytes());
    bytes.extend_from_slice(&window.to_be_bytes());
    bytes
}

/// (kind, tunnel_id, consumed, window), если это корректный блок окна.
pub(crate) fn decode_window(bytes: &[u8]) -> Option<(u8, u64, u64, u32)> {
    if !is_tunnel(bytes) || bytes.len() != HEADER + 4 || bytes[1] != VERSION || bytes[18] != 2 {
        return None;
    }
    validate(bytes).ok()?;
    Some((
        bytes[0],
        u64::from_be_bytes(bytes[2..10].try_into().ok()?),
        u64::from_be_bytes(bytes[10..18].try_into().ok()?),
        u32::from_be_bytes(bytes[HEADER..HEADER + 4].try_into().ok()?),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{proxy::ProxyTunnelData, socks5::Socks5TunnelData};

    fn block(kind: u8, data: Vec<u8>, offset: u64, close: bool) -> Vec<u8> {
        if kind == SOCKS5 {
            Socks5TunnelData {
                tunnel_id: 42,
                byte_offset: offset,
                data,
                close,
            }
            .encode()
        } else {
            ProxyTunnelData {
                tunnel_id: 42,
                byte_offset: offset,
                data,
                close,
            }
            .encode()
        }
    }
    fn parse(bytes: &[u8], kind: u8) -> Option<(u64, u64, bool, Vec<u8>)> {
        if kind == SOCKS5 {
            Socks5TunnelData::parse(bytes).map(|b| (b.tunnel_id, b.byte_offset, b.close, b.data))
        } else {
            ProxyTunnelData::parse(bytes).map(|b| (b.tunnel_id, b.byte_offset, b.close, b.data))
        }
    }

    #[test]
    fn tunnel_v1_header_roundtrip_and_protocols_are_disjoint() {
        for kind in [SOCKS5, HTTP_CONNECT] {
            let wire = block(kind, vec![1, 2, 3], 7, false);
            let mut expected = vec![kind, 1];
            expected.extend_from_slice(&42u64.to_be_bytes());
            expected.extend_from_slice(&7u64.to_be_bytes());
            expected.push(0);
            expected.extend_from_slice(&3u32.to_be_bytes());
            expected.extend_from_slice(&[1, 2, 3]);
            assert_eq!(wire, expected);
            assert_eq!(parse(&wire, kind), Some((42, 7, false, vec![1, 2, 3])));
            assert!(parse(&wire, if kind == SOCKS5 { HTTP_CONNECT } else { SOCKS5 }).is_none());
            assert_eq!(
                parse(&block(kind, vec![], u64::MAX, true), kind),
                Some((42, u64::MAX, true, vec![]))
            );
        }
    }

    #[test]
    fn tunnel_v0_v2_rejected_before_lengths_payload_and_queue_allocation() {
        let stats = super::super::transport::get_wagon_stats();
        let before = stats.tunnel_version_rejected.load(Ordering::Relaxed);
        let queue = (
            stats.tunnel_queue_depth.load(Ordering::Relaxed),
            stats.tunnel_queue_bytes.load(Ordering::Relaxed),
            stats.tunnel_reorder_depth.load(Ordering::Relaxed),
            stats.tunnel_reorder_bytes.load(Ordering::Relaxed),
        );
        for kind in [SOCKS5, HTTP_CONNECT] {
            for version in [0, 2] {
                // Even an incomplete header is rejected by version before reading lengths.
                assert!(reject_invalid(&[kind, version]));
                let mut wire = block(kind, vec![1], 0, false);
                wire[1] = version;
                wire[19..23].copy_from_slice(&u32::MAX.to_be_bytes());
                assert!(parse(&wire, kind).is_none());
            }
        }
        assert_eq!(
            stats.tunnel_version_rejected.load(Ordering::Relaxed),
            before + 8
        );
        assert_eq!(
            queue,
            (
                stats.tunnel_queue_depth.load(Ordering::Relaxed),
                stats.tunnel_queue_bytes.load(Ordering::Relaxed),
                stats.tunnel_reorder_depth.load(Ordering::Relaxed),
                stats.tunnel_reorder_bytes.load(Ordering::Relaxed)
            )
        );
    }

    #[test]
    fn tunnel_truncated_extra_bytes_flags_and_overflow_rejected() {
        for kind in [SOCKS5, HTTP_CONNECT] {
            let wire = block(kind, vec![1, 2, 3], 0, false);
            for len in 0..wire.len() {
                assert!(parse(&wire[..len], kind).is_none(), "prefix length {len}");
            }
            let mut bad = wire.clone();
            bad.push(0);
            assert!(parse(&bad, kind).is_none());
            for flag in [1, 2, 255] {
                let mut bad = wire.clone();
                bad[18] = flag;
                assert!(parse(&bad, kind).is_none());
            }
            assert!(parse(&block(kind, vec![1], u64::MAX, false), kind).is_none());
        }
    }

    #[test]
    fn tunnel_legacy_forms_are_not_accepted() {
        let mut old_socks = vec![0xD7];
        old_socks.extend_from_slice(&42u64.to_be_bytes());
        old_socks.extend_from_slice(&[0, 1, 2, 3]);
        let old_http = bincode::serialize(&(42u64, vec![1u8, 2, 3], false)).unwrap();
        let mut old_http_offset = b"YANDISO2".to_vec();
        old_http_offset.extend_from_slice(&0u64.to_be_bytes());
        old_http_offset.extend_from_slice(&old_http);
        let mut old_socks_offset = vec![0xD8];
        old_socks_offset.extend_from_slice(&42u64.to_be_bytes());
        old_socks_offset.extend_from_slice(&[0xFF; 8]);
        old_socks_offset.extend_from_slice(&0u64.to_be_bytes());
        old_socks_offset.extend_from_slice(&[0, 1, 2, 3]);
        for wire in [
            old_socks,
            old_http,
            old_http_offset,
            old_socks_offset,
            br#"{"tunnel_id":42,"data":[1,2,3],"close":false,"byte_offset":0}"#.to_vec(),
        ] {
            assert!(Socks5TunnelData::parse(&wire).is_none());
            assert!(ProxyTunnelData::parse(&wire).is_none());
        }
    }

    #[test]
    fn window_block_roundtrip_is_not_data_and_is_validated() {
        for kind in [SOCKS5, HTTP_CONNECT] {
            let w = encode_window(kind, 9, 123_456, 1 << 20);
            assert_eq!(decode_window(&w), Some((kind, 9, 123_456, 1 << 20)));
            assert!(decode(&w, kind).is_none(), "окно не должно приниматься как данные");
            assert!(validate(&w).is_ok());
            // обрезанное, лишние байты, неверная длина, чужая версия
            assert!(decode_window(&w[..w.len() - 1]).is_none());
            let mut longer = w.clone();
            longer.push(0);
            assert!(decode_window(&longer).is_none());
            let mut bad_len = w.clone();
            bad_len[22] = 5;
            assert!(decode_window(&bad_len).is_none());
            let mut v2 = w.clone();
            v2[1] = 2;
            assert!(decode_window(&v2).is_none());
            // обычный блок данных и close не окно
            let data = encode(kind, 9, 0, false, &[1, 2]);
            assert!(decode_window(&data).is_none());
        }
    }
}
