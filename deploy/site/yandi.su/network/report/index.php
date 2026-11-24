<?php
$pageTitle = 'YANDI — Report a bug'; $pageDescription = 'How to report a bug in a YANDI test build: what to write, which versions to name, and what never to attach (passwords, seed phrase, keys, private chats).'; $activeNav = 'report';
require __DIR__ . '/../../includes/header.php';
?>
<section class="hero"><div class="container"><h1>Report a bug</h1><p class="lead">The current builds are test versions. They exist to find bugs, so your report is the point of testing.</p></div></section>
<section><div class="container"><div class="download-detail">
<div class="notice"><strong>Never attach secrets.</strong> Do not send passwords, the seed phrase (12 or 24 words), recovery codes, key files (<code>~/.yandi_keys</code>), backup files (<code>.yandibak</code>), pairing codes or QR images, or your private messages, photos and files. A report never needs them; if someone asks for them, do not send them.</div>
<div class="download-steps"><h3>How to send a report</h3>
<ol>
<li>Write to <a href="mailto:support@yandi.su?subject=%5BYANDI%20test%5D%20">support@yandi.su</a> with the subject starting with <code>[YANDI test]</code>.</li>
<li>Say <strong>what you did</strong>, step by step, what you <strong>expected</strong> and what <strong>happened</strong> instead (the exact error text if there is one).</li>
<li>Give the <strong>versions</strong> from the download page: the node and shell package (for example <code>0.2.0+g819</code>) and the Android app (for example <code>1.0.0, build 1</code>).</li>
<li>Name the <strong>system</strong> (Debian or Ubuntu release; phone model and Android version) and the <strong>network</strong> (home Wi-Fi, mobile network, proxy) when it matters.</li>
<li>Say whether the problem repeats every time or happened once, and roughly <strong>when</strong> it happened (date and time).</li>
<li>Screenshots are welcome: hide names, numbers, addresses and anything private before sending. Do not send logs unless you are asked to, and then check them for private data first.</li>
</ol></div>
<div class="download-steps"><h3>Security problems</h3><p>If you think you found a weakness (for example a way to read someone else’s messages or to get into a node), write to the same address with the subject <code>[YANDI security]</code> and do not publish the details until it is fixed. Describe the problem, not a working attack.</p></div>
<p><a href="/network/">← Back to downloads</a></p>
</div></div></section>
<?php require __DIR__ . '/../../includes/footer.php'; ?>
