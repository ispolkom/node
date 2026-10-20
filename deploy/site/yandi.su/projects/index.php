<?php
$pageTitle = 'YANDI — Open Projects'; $pageDescription = 'The public part of the YANDI ecosystem and its open repositories.'; $activeNav = 'projects';
require __DIR__ . '/../includes/header.php';
?>
<section class="hero"><div class="container"><h1>Open Projects</h1><p class="lead">Public repositories represent only part of the YANDI ecosystem.</p><p>Some projects are in development and have not been published. The GitHub list should therefore not be treated as a complete catalog of YANDI.</p></div></section>
<section><div class="container"><h2>Projects</h2><div class="grid">
<article class="card"><h3>YANDI Node</h3><p>The node of the private network: chats, files, groups, voice and video calls, access to the internet through trusted nodes. Written in Rust.</p><p class="download-status">Status: test version (0.2.0). Repository: <a href="https://github.com/ispolkom/node" rel="noopener noreferrer">github.com/ispolkom/node</a></p><a class="card-link" href="/network/">Downloads</a></article>
<article class="card"><h3>YANDI apps</h3><p>The Android app and the desktop shell (Debian, Ubuntu) that work with your node. They live in the same repository as the node.</p><p class="download-status">Status: test version. Windows, macOS and iOS: not built yet.</p><a class="card-link" href="/network/">Downloads</a></article>
</div></div></section>
<section><div class="container"><div class="card"><h2>GitHub</h2><p>The public part of the development is available in the YANDI organization:</p><p><a href="https://github.com/ispolkom" rel="noopener noreferrer">github.com/ispolkom →</a></p><p class="muted">Individual directions may have different statuses: open project, prototype, research or development.</p></div></div></section>
<?php require __DIR__ . '/../includes/footer.php'; ?>
