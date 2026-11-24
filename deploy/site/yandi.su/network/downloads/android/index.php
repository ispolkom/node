<?php
$downloadPlatform = 'Android'; $downloadIcon = '/assets/icons/android.svg'; $downloadRelease = 'android';
$downloadStatus = 'Test build available';
$downloadDescription = 'The YANDI mobile app: chats, files, groups and calls through your trusted YANDI node. Distributed as an APK file, outside of app stores.';
$downloadSteps = ['Download <code>yandi.apk</code> on your phone (or download it on a computer and copy it to the phone).', 'Open the file. If Android asks, allow installing apps from this source: <em>Settings → Apps → Special access → Install unknown apps</em>, then pick your browser or file manager.', 'Confirm the installation. If Play Protect warns about an unknown app, choose “Install anyway”: the app is not distributed through Google Play.', 'Open YANDI and set a password.', 'Pair the phone with your node: show the pairing code (QR) in the YANDI shell on your computer (the paired devices section) and scan or enter it in the app.'];
$downloadNotes = ['The phone needs a YANDI node to work with: your own or a trusted one. See the Debian page to run a node on a computer.', 'Updating: install the newer <code>yandi.apk</code> over the old one, the data is kept. The app also tells you about new versions through your node.', 'To verify the file, compare the output of <code>sha256sum yandi.apk</code> with the sum above.'];
require __DIR__ . '/../_page.php';
