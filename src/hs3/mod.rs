//! Рукопожатие версии 3 — **исследовательская модель, не подключена к узлу** (этап 2б-1, docs/HANDSHAKE_HARDENING.md).
//!
//! Чистый код без ввода-вывода: кодек, расписание ключей и (позже) конечный автомат. Нужен, чтобы доказать свойства конструкции моделью и
//! независимыми векторами до того, как что-либо будет встроено в рабочую сеть. Рабочие пути (`netlayer`, `p2p`) этот модуль не вызывают.
pub mod codec;
pub mod kdf;

#[cfg(test)]
mod tests {
    use super::codec::*;
    use super::kdf;
    use ed25519_dalek::SigningKey;
    use serde_json::Value;
    use x25519_dalek::{PublicKey, StaticSecret};

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../tests/hs3_vectors.json")).unwrap()
    }
    fn hx<const N: usize>(v: &Value, k: &str) -> [u8; N] {
        let b = hex::decode(v[k].as_str().unwrap()).unwrap();
        let mut a = [0u8; N];
        a.copy_from_slice(&b);
        a
    }
    fn hv(v: &Value, k: &str) -> Vec<u8> {
        hex::decode(v[k].as_str().unwrap()).unwrap()
    }

    fn build() -> (Value, Request, Ack, Vec<u8>, Vec<u8>) {
        let v = vectors();
        let isk = SigningKey::from_bytes(&hx::<32>(&v, "initiator_seed"));
        let rsk = SigningKey::from_bytes(&hx::<32>(&v, "responder_seed"));
        let ea = PublicKey::from(&StaticSecret::from(hx::<32>(&v, "ea_secret"))).to_bytes();
        let eb = PublicKey::from(&StaticSecret::from(hx::<32>(&v, "eb_secret"))).to_bytes();
        let req = Request {
            context: CTX_NETLAYER_UDP,
            node_id: hx(&v, "initiator_id"),
            signing_pub: isk.verifying_key().to_bytes(),
            ea,
            time_ms: 1_700_000_000_000,
            nonce: [0xAA; 16],
            expected_peer: hx(&v, "responder_id"),
            capabilities: 0x0F,
            jurisdiction: "RU".into(),
            name: "alice".into(),
        };
        let r = req.encode(&isk);
        let ack = Ack {
            context: CTX_NETLAYER_UDP,
            node_id: hx(&v, "responder_id"),
            signing_pub: rsk.verifying_key().to_bytes(),
            eb,
            time_ms: 1_700_000_001_000,
            nonce: [0xBB; 16],
            h_r: kdf::hash_request(&r),
            expected_initiator: hx(&v, "initiator_id"),
            capabilities: 0x03,
            jurisdiction: "NL".into(),
            name: "bob".into(),
        };
        let a = ack.encode(&rsk);
        (v, req, ack, r, a)
    }

    /// Все байты и ключи совпадают с независимой реализацией на Python (tools/hs3_vectors.py).
    #[test]
    fn rust_matches_the_independent_python_vectors() {
        let (v, req, ack, r, a) = build();
        assert_eq!(r, hv(&v, "request"), "запрос");
        assert_eq!(a, hv(&v, "ack"), "ответ");
        assert_eq!(kdf::hash_request(&r), hx::<32>(&v, "h_r"));
        let t = kdf::transcript(&r, &a);
        assert_eq!(t, hx::<32>(&v, "transcript"));
        let dh = StaticSecret::from(hx::<32>(&v, "ea_secret")).diffie_hellman(&PublicKey::from(ack.eb));
        assert_eq!(dh.to_bytes(), hx::<32>(&v, "dh"));
        let keys = kdf::derive(&t, &dh.to_bytes(), CTX_NETLAYER_UDP).unwrap();
        assert_eq!(*keys.i2r, hx::<32>(&v, "i2r"));
        assert_eq!(*keys.r2i, hx::<32>(&v, "r2i"));
        assert_eq!(*keys.kc_i, hx::<32>(&v, "kc_i"));
        assert_eq!(*keys.kc_r, hx::<32>(&v, "kc_r"));
        assert_eq!(*keys.exporter, hx::<32>(&v, "exporter"));
        let fi = Fin { kind: MsgType::FinI, context: CTX_NETLAYER_UDP, t, mac: kdf::fin_mac(&keys.kc_i, true, &t) }.encode();
        let fr = Fin { kind: MsgType::FinR, context: CTX_NETLAYER_UDP, t, mac: kdf::fin_mac(&keys.kc_r, false, &t) }.encode();
        assert_eq!(fi, hv(&v, "fin_i"));
        assert_eq!(fr, hv(&v, "fin_r"));
        let fk = kdf::file_key(&keys.exporter, &hv(&v, "file_id")).unwrap();
        assert_eq!(*fk, hx::<32>(&v, "file_key"));
        // разбор тех же байт возвращает те же поля
        assert_eq!(Request::decode(&r).unwrap(), req);
        assert_eq!(Ack::decode(&a).unwrap(), ack);
        assert_eq!(r.len(), v["sizes"]["request"].as_u64().unwrap() as usize);
        assert_eq!(a.len(), v["sizes"]["ack"].as_u64().unwrap() as usize);
        assert_eq!(fi.len(), FIN_LEN);
    }

    #[test]
    fn maximum_sizes_match_the_schema_arithmetic() {
        let (_v, mut req, mut ack, _, _) = build();
        let isk = SigningKey::from_bytes(&[1u8; 32]);
        let rsk = SigningKey::from_bytes(&[2u8; 32]);
        req.jurisdiction = "J".repeat(MAX_JURISDICTION);
        req.name = "N".repeat(MAX_NAME);
        ack.jurisdiction = "J".repeat(MAX_JURISDICTION);
        ack.name = "N".repeat(MAX_NAME);
        assert_eq!(req.encode(&isk).len(), MAX_REQUEST);
        assert_eq!(ack.encode(&rsk).len(), MAX_ACK);
        assert_eq!(KEY_CONFIRM_LEN, 57);
    }

    #[test]
    fn the_decoder_is_strict() {
        let (_v, _req, _ack, r, a) = build();
        // лишний байт в конце
        let mut x = r.clone();
        x.push(0);
        assert_eq!(Request::decode(&x), Err(CodecError::Trailing));
        // усечение
        assert_eq!(Request::decode(&r[..r.len() - 1]), Err(CodecError::Short));
        // зарезервированные флаги, версия, набор, контекст, тип, магия
        for (idx, val, err) in [(8usize, 1u8, CodecError::Flags), (4, 2, CodecError::Version), (6, 2, CodecError::Suite), (7, 9, CodecError::Context), (5, 9, CodecError::Type), (0, b'X', CodecError::Magic)] {
            let mut y = r.clone();
            y[idx] = val;
            assert_eq!(Request::decode(&y), Err(err), "байт {idx}");
        }
        // тип ответа вместо запроса
        assert_eq!(Request::decode(&a), Err(CodecError::UnexpectedType));
        // испорченная подпись и подмена поля без переподписи
        let mut z = r.clone();
        let n = z.len();
        z[n - 1] ^= 1;
        assert_eq!(Request::decode(&z), Err(CodecError::BadSignature));
        let mut w = r.clone();
        w[HEADER_LEN + 64 + 3] ^= 1; // байт Ea
        assert_eq!(Request::decode(&w), Err(CodecError::BadSignature));
        // чужой node_id при верной подписи ключа
        let mut u = r.clone();
        u[HEADER_LEN] ^= 1;
        assert_eq!(Request::decode(&u), Err(CodecError::NodeIdMismatch));
    }

    #[test]
    fn strings_are_validated() {
        let (_v, mut req, _ack, _, _) = build();
        let isk = SigningKey::from_bytes(&[1u8; 32]);
        req.jurisdiction = String::new();
        req.name = String::new();
        let ok = req.encode(&isk);
        assert_eq!(Request::decode(&ok).unwrap(), req, "пустые строки допустимы");
        // невалидный UTF-8 в имени (с корректной подписью над испорченными байтами строится вручную)
        let mut body = ok[..ok.len() - 64].to_vec();
        let name_len_pos = body.len() - 1;
        body[name_len_pos] = 2;
        body.extend_from_slice(&[0xff, 0xfe]);
        use ed25519_dalek::Signer;
        let sig = isk.sign(&[b"yandi-hs-v3-req".as_slice(), &body].concat());
        body.extend_from_slice(&sig.to_bytes());
        assert_eq!(Request::decode(&body), Err(CodecError::Utf8));
        // слишком длинная строка юрисдикции
        let mut body2 = ok[..ok.len() - 64].to_vec();
        let jpos = body2.len() - 2;
        body2[jpos] = (MAX_JURISDICTION + 1) as u8;
        body2.extend(std::iter::repeat(b'J').take(MAX_JURISDICTION + 1));
        body2.push(0); // длина имени 0
        let sig2 = isk.sign(&[b"yandi-hs-v3-req".as_slice(), &body2].concat());
        body2.extend_from_slice(&sig2.to_bytes());
        assert_eq!(Request::decode(&body2), Err(CodecError::StringTooLong));
    }

    #[test]
    fn fin_and_key_confirm_frames_are_exact() {
        let t = [7u8; 32];
        let f = Fin { kind: MsgType::FinI, context: CTX_P2P_CHAT, t, mac: [9u8; 32] };
        let b = f.encode();
        assert_eq!(Fin::decode(&b).unwrap(), f);
        let mut long = b.clone();
        long.push(0);
        assert_eq!(Fin::decode(&long), Err(CodecError::Trailing));
        assert_eq!(Fin::decode(&b[..b.len() - 1]), Err(CodecError::Short));
        let kc = key_confirm_frame(CTX_NETLAYER_WS, &t, &[3u8; 16]);
        assert_eq!(kc.len(), KEY_CONFIRM_LEN);
        assert_eq!(parse_key_confirm(&kc).unwrap(), (CTX_NETLAYER_WS, t, [3u8; 16]));
        assert_eq!(Fin::decode(&kc), Err(CodecError::UnexpectedType));
    }

    #[test]
    fn a_weak_shared_secret_is_refused_before_any_key_is_derived() {
        assert!(kdf::derive(&[1u8; 32], &[0u8; 32], CTX_NETLAYER_UDP).is_none());
        assert!(kdf::file_key(&[1u8; 32], &[0u8; 65]).is_none(), "file_id длиннее 64 байт отвергается");
    }

    #[test]
    fn keys_depend_on_the_transcript_the_direction_and_the_context() {
        let dh = [5u8; 32];
        let a = kdf::derive(&[1u8; 32], &dh, CTX_NETLAYER_UDP).unwrap();
        let b = kdf::derive(&[2u8; 32], &dh, CTX_NETLAYER_UDP).unwrap();
        let c = kdf::derive(&[1u8; 32], &dh, CTX_NETLAYER_WS).unwrap();
        assert_ne!(*a.i2r, *b.i2r, "другой транскрипт — другие ключи");
        assert_ne!(*a.i2r, *c.i2r, "другой контекст — другие ключи");
        assert_ne!(*a.i2r, *a.r2i, "направления различны");
        assert_ne!(*a.kc_i, *a.kc_r);
        let t = [1u8; 32];
        assert_ne!(kdf::fin_mac(&a.kc_i, true, &t), kdf::fin_mac(&a.kc_i, false, &t), "метка роли входит в MAC");
        assert!(kdf::mac_eq(&kdf::fin_mac(&a.kc_i, true, &t), &kdf::fin_mac(&a.kc_i, true, &t)));
    }
}
