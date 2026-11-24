# YANDI background delivery without Google

YANDI does not use FCM or any other mandatory Google service. On Android, delivery is handled by a dedicated foreground service that keeps one TLS/WebSocket connection to the selected node. The node forwards encrypted message and call-signal frames; the native layer does not decrypt their contents.

The service uses an adaptive power mode:

- screen on: a service ping every 30 seconds;
- screen off: a ping every 180 seconds;
- Doze: a ping every 300 seconds, plus the system TCP keepalive;
- when the network is lost, the connection is closed and restored after `NetworkCallback`;
- after an Android reboot, the saved configuration restores the service without Flutter;
- unacknowledged call signals are stored locally in a bounded queue and passed to Flutter when the app opens.

This provides independent push-like delivery with reasonable battery use. Android cannot guarantee delivery “without a process and without battery”: the operating system does not provide third-party applications with an independent wake-up channel. YANDI therefore keeps a reliable delivery mode for calls and messages instead of imitating push through frequent timers.

The next separate step is a dedicated push relay. It could deliver a short encrypted wake envelope through independent infrastructure, such as a self-hosted UnifiedPush distributor. The app would then open its normal TLS connection to the node and fetch the data. The relay must not receive message text, keys, or phone numbers. Until that infrastructure exists, the current WebSocket foreground service remains the primary transport: it already works without Google and does not require an external push provider.
