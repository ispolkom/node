//! "Valid hostile peer" resource-exhaustion probe + regression test.
//!
//! Not forged packets, not replay — a peer that completes a real,
//! correctly-signed handshake and sends a real, correctly-typed,
//! correctly-authenticated protocol message, but with a maliciously
//! chosen field value. Transport-layer authentication ("we know who
//! sent this") was already hardened this session (identity binding +
//! replay protection, node/tests/{hello_replay_test,p2p_hello_replay_test}.rs)
//! — this test is about the SEPARATE, downstream question: does the
//! receiving application logic trust attacker-controlled CONTENT inside
//! an authenticated message?
//!
//! `communication::FileTransferManager::start_receiving` is exercised
//! directly with a real `FileChunkStart` (the genuine wire type) built
//! the same way `communication::chat::handle_comm_packet` builds one
//! after decrypting and deserializing a real peer's packet — this is
//! the exact code path a real network message reaches, this test just
//! doesn't re-run the UDP+handshake+encryption hop in front of it
//! (that hop is what the earlier replay/identity tests already cover).
//!
//! BEFORE the fix in this same commit: a 133-byte message with
//! `total_chunks = u32::MAX` forced an immediate ~4 GB memory
//! reservation. AFTER: it's rejected outright, and legitimate transfers
//! (correct, self-consistent file_size/total_chunks, under the new cap)
//! are unaffected.
//!
//! All scenarios share ONE transport/port pair: p2p::P2PTransport's
//! ports come from process-global env vars (YANDI_P2P_*), not
//! constructor params — running them as separate #[tokio::test] fns
//! would race each other over those vars under cargo test's default
//! parallelism (same reason src/communication/chat.rs's own tests do
//! this).

use yandi::communication::{FileChunkStart, FileTransferManager, FILE_TRANSFER_CHUNK_SIZE};
use yandi::core::identity::NodeIdentity;
use yandi::p2p::P2PTransport;
use yandi::util::HashId;

fn vm_size_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmSize:") {
            return rest.trim().trim_end_matches(" kB").trim().parse().unwrap_or(0);
        }
    }
    0
}

fn expected_chunks(file_size: u64) -> u32 {
    ((file_size as usize + FILE_TRANSFER_CHUNK_SIZE - 1) / FILE_TRANSFER_CHUNK_SIZE) as u32
}

#[tokio::test]
async fn valid_peer_file_transfer_content_validation() {
    // файлы узла — в папке данных; у теста она временная (не папка владельца)
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", data.path());
    std::env::set_var("YANDI_P2P_DISCOVERY_PORT", "19601");
    std::env::set_var("YANDI_P2P_DATA_PORT", "19602");
    let target = P2PTransport::new(NodeIdentity::new(), 0).await.expect("start target transport");
    let ftm = FileTransferManager::new(target.node_id(), target);

    // ── 1. The exploit: total_chunks wildly inconsistent with file_size,
    //    and both absurdly large. Must be rejected before anything is
    //    allocated. ──
    {
        let attacker_id = HashId::new_random();
        let vmsize_before = vm_size_kb();

        let malicious_start = FileChunkStart {
            file_id: "attacker-file-1".to_string(),
            filename: "definitely-not-a-bomb.txt".to_string(),
            file_size: 1,
            mime_type: "text/plain".to_string(),
            total_chunks: u32::MAX,
        };
        let wire_size = serde_json::to_vec(&malicious_start).unwrap().len();

        let result = ftm.start_receiving(attacker_id, malicious_start).await;

        let vmsize_after = vm_size_kb();
        let vmsize_delta_mb = vmsize_after.saturating_sub(vmsize_before) / 1024;

        println!(
            "wire message: {} bytes -> VmSize delta: {} MB, start_receiving result: {:?}",
            wire_size, vmsize_delta_mb, result,
        );

        assert!(
            result.is_err(),
            "RESOURCE EXHAUSTION: a {}-byte message with total_chunks=u32::MAX was accepted",
            wire_size
        );
        assert!(
            vmsize_delta_mb < 10,
            "RESOURCE EXHAUSTION: request was rejected but still grew VmSize by {} MB before rejecting",
            vmsize_delta_mb
        );
    }

    // ── 2. A total_chunks/file_size mismatch must be rejected even when
    //    both values are individually small — the fix checks
    //    consistency, not just an upper bound on one field. ──
    {
        let attacker_id = HashId::new_random();
        let start = FileChunkStart {
            file_id: "attacker-file-2".to_string(),
            filename: "small-but-wrong.txt".to_string(),
            file_size: 10, // needs 1 chunk of 700B
            mime_type: "text/plain".to_string(),
            total_chunks: 500, // way more than needed for 10 bytes
        };
        let result = ftm.start_receiving(attacker_id, start).await;
        assert!(result.is_err(), "a total_chunks/file_size mismatch must be rejected even when both values are individually small");
    }

    // ── 3. A file right at the size cap, with correctly-computed
    //    total_chunks (mirroring the real sender's own formula), must
    //    still be accepted — the fix must not break legitimate transfers. ──
    {
        let peer_id = HashId::new_random();
        const MAX_FILE_TRANSFER_SIZE: u64 = 200 * 1024 * 1024;
        let file_size = MAX_FILE_TRANSFER_SIZE;
        let start = FileChunkStart {
            file_id: "legit-large-file".to_string(),
            filename: "real-attachment.bin".to_string(),
            file_size,
            mime_type: "application/octet-stream".to_string(),
            total_chunks: expected_chunks(file_size),
        };
        let result = ftm.start_receiving(peer_id, start).await;
        assert!(result.is_ok(), "a legitimate, self-consistent transfer at the size cap must still be accepted: {:?}", result);
    }

    // ── 4. An ordinary small file (a typical real chat attachment) must
    //    work exactly as before — the common case, not just the edge case. ──
    {
        let peer_id = HashId::new_random();
        let file_size: u64 = 12_345;
        let start = FileChunkStart {
            file_id: "legit-small-file".to_string(),
            filename: "photo.jpg".to_string(),
            file_size,
            mime_type: "image/jpeg".to_string(),
            total_chunks: expected_chunks(file_size),
        };
        let result = ftm.start_receiving(peer_id, start).await;
        assert!(result.is_ok(), "an ordinary small chat attachment must still be accepted: {:?}", result);
    }
}
