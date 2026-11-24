//! Verify the production YSE1 boundary for node-to-node content.

use ed25519_dalek::SigningKey;
use yandi::communication::ChatMessage;
use yandi::core::NodeIdentity;
use yandi::crypto::sealed;

#[test]
fn node_chat_content_is_sealed_for_the_recipient_before_transport() {
    let alice = NodeIdentity::new();
    let bob = NodeIdentity::new();
    let relay = NodeIdentity::new();
    let marker = "synthetic_e2e_boundary_marker_7391";
    let message = ChatMessage::new(alice.node_id(), bob.node_id(), marker.into());
    let application_packet = serde_json::to_vec(&message).unwrap();
    assert!(application_packet.windows(marker.len()).any(|part| part == marker.as_bytes()));
    let sender = SigningKey::from_bytes(&alice.signing_private_key);
    let recipient = SigningKey::from_bytes(&bob.signing_private_key);
    let intermediary = SigningKey::from_bytes(&relay.signing_private_key);
    let created_ms = 1_800_000_000_000;
    let envelope = sealed::seal(&sender, &recipient.verifying_key(), &alice.node_id().0, &bob.node_id().0, created_ms, &application_packet).unwrap();
    assert!(!envelope.windows(marker.len()).any(|part| part == marker.as_bytes()));
    assert!(sealed::open(&intermediary, &sender.verifying_key(), &alice.node_id().0, &bob.node_id().0, created_ms, &envelope).is_err());
    assert!(sealed::open(&recipient, &intermediary.verifying_key(), &alice.node_id().0, &bob.node_id().0, created_ms, &envelope).is_err());
    let (_, opened) = sealed::open(&recipient, &sender.verifying_key(), &alice.node_id().0, &bob.node_id().0, created_ms, &envelope).unwrap();
    let received: ChatMessage = serde_json::from_slice(&opened).unwrap();
    assert_eq!(received.text, marker);
}
