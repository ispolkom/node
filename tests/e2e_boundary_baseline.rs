//! Baseline witness for the current PC-node application boundary.
//! It deliberately records the gap; transport encryption still protects the link.

use yandi::communication::ChatMessage;
use yandi::util::HashId;

#[test]
fn node_chat_application_packet_contains_readable_text_before_transport_encryption() {
    let marker = "synthetic_e2e_boundary_marker_7391";
    let message = ChatMessage::new(HashId::default(), HashId::default(), marker.into());
    let application_packet = serde_json::to_vec(&message).unwrap();
    assert!(application_packet.windows(marker.len()).any(|part| part == marker.as_bytes()));
}
