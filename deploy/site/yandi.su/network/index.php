<?php
$pageTitle = 'YANDI Network — private P2P network, download for Android and Linux';
$pageDescription = 'YANDI Network is a private P2P network of your own and trusted nodes: encrypted chats, files and calls. Download the test build for Android, Debian and Ubuntu.';
$activeNav = 'network';
require __DIR__ . '/../includes/header.php';
?>
<section class="hero"><div class="container"><h1>YANDI Network</h1><p class="lead">A private community network built by its participants.</p><p>YANDI Network is one direction of the YANDI ecosystem, not the whole of YANDI. Participants can run their own nodes and connect trusted devices and communities.</p></div></section>
<section class="downloads-section"><div class="container"><h2>Downloads</h2><div class="notice"><strong>Test version.</strong> This version is a test build. It is meant for checking the software and finding bugs. Do not rely on it for important data or critical communication, and please report any problems you find.</div><p>YANDI client downloads and platform resources. Test builds are available for Android, Debian and Ubuntu; other platforms are not built yet.</p><div class="download-grid">
<article class="card download-card is-ready"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/android.svg" alt="Android"></span><div><h3>Android</h3><p class="download-status">Test build available</p></div><a class="card-link" href="/network/downloads/android/">Details</a></article>
<article class="card download-card is-ready"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/debian.svg" alt="Debian"></span><div><h3>Debian</h3><p class="download-status">Test build available</p></div><a class="card-link" href="/network/downloads/debian/">Details</a></article>
<article class="card download-card is-ready"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/ubuntu.svg" alt="Ubuntu"></span><div><h3>Ubuntu</h3><p class="download-status">Test build available</p></div><a class="card-link" href="/network/downloads/ubuntu/">Details</a></article>
<article class="card download-card"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/windows.svg" alt="Windows"></span><div><h3>Windows</h3><p class="download-status">Not available yet</p></div><a class="card-link" href="/network/downloads/windows/">Details</a></article>
<article class="card download-card"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/macos.svg" alt="macOS"></span><div><h3>macOS</h3><p class="download-status">Not available yet</p></div><a class="card-link" href="/network/downloads/macos/">Details</a></article>
<article class="card download-card"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/apple.svg" alt="iPhone / iPad"></span><div><h3>iPhone / iPad</h3><p class="download-status">Not available yet</p></div><a class="card-link" href="/network/downloads/apple/">Details</a></article>
</div><p><a class="btn" href="/network/report/">Report a bug</a></p></div></section>
<section><div class="container"><h2>Network principles</h2><div class="grid">
<article class="card"><h3>Your nodes</h3><p>Nodes belong to participants. Users choose the devices and nodes they trust.</p></article>
<article class="card"><h3>Direct communication</h3><p>P2P connections, private messages, file transfer, audio calls and video communication.</p></article>
<article class="card"><h3>Privacy</h3><p>E2E communication, invitation-based membership and no mandatory public user directory.</p></article>
<article class="card"><h3>No single mandatory server</h3><p>There is no single YANDI server that everyone must trust. The architecture allows personal trust circles and multiple nodes.</p></article>
</div></div></section>
<section><div class="container"><h2>Mobile access</h2><p>A phone does not have to be a permanently available full P2P node. It can connect to a trusted node operated by the user or the community:</p><p><strong>Phone → trusted node → other community nodes</strong></p><p>This does not make that node a mandatory central YANDI server. The user chooses it, and multiple trusted nodes can be used for resilience in the future.</p></div></section>
<?php require __DIR__ . '/../includes/footer.php'; ?>
