<?php
require_once __DIR__ . '/../../includes/releases.php';
$pageTitle = 'Download YANDI for ' . $downloadPlatform . ' — test build';
$pageDescription = 'Download YANDI for ' . $downloadPlatform . ': installation instructions, version, release date and checksums. ' . $downloadStatus . '. Test build for finding bugs.';
$activeNav = 'network';
require __DIR__ . '/../../includes/header.php';
$rel = $downloadRelease !== null ? (YANDI_RELEASES[$downloadRelease] ?? null) : null;
?>
<section class="hero"><div class="container"><h1><?= e($downloadPlatform) ?></h1><p class="lead"><?= e($downloadStatus) ?></p></div></section>
<section><div class="container"><div class="download-detail">
    <div class="notice"><strong>Test version.</strong> This version is a test build. It is meant for checking the software and finding bugs. Do not rely on it for important data or critical communication, and please report any problems you find.</div>
    <span class="platform-icon-shell"><img class="platform-icon" src="<?= e($downloadIcon) ?>" alt="<?= e($downloadPlatform) ?>"></span>
    <h2>YANDI Network for <?= e($downloadPlatform) ?></h2>
    <p><?= e($downloadDescription) ?></p>
    <?php if ($rel): ?>
        <div class="file-box">
        <?php foreach ($rel['files'] as $f): ?>
            <h3><?= e($f['name']) ?></h3>
            <dl><dt>Version</dt><dd><?= e($rel['version']) ?></dd><dt>Release date</dt><dd><?= e($rel['date']) ?></dd><dt>Size</dt><dd><?= e($f['size']) ?> MB</dd><dt>SHA-256</dt><dd><code><?= e($f['sha256']) ?></code></dd></dl>
            <a class="btn big" href="<?= e($f['href']) ?>" download>Download <?= e($f['name']) ?></a>
        <?php endforeach; ?>
        </div>
    <?php else: ?>
        <p class="muted">There is no build for this platform yet.</p>
    <?php endif; ?>
    <?php if ($rel): ?>
    <div class="download-steps"><h3>What is new</h3><ul><?php foreach ($rel['changes']['en'] as $c): ?><li><?= e($c) ?></li><?php endforeach; ?></ul>
    <h3>Compatibility</h3><p><?= e($rel['compat']['en']) ?></p></div>
    <?php endif; ?>
    <div class="download-steps"><h3>Installation</h3><ol><?php foreach ($downloadSteps as $step): ?><li><?= $step ?></li><?php endforeach; ?></ol></div>
    <?php if (!empty($downloadNotes)): ?><div class="download-steps"><h3>Notes</h3><ul><?php foreach ($downloadNotes as $n): ?><li><?= $n ?></li><?php endforeach; ?></ul></div><?php endif; ?>
    <p><a class="btn" href="/network/report/">Report a bug</a></p>
    <p><a href="/network/">← Back to downloads</a></p>
</div></div></section>
<?php require __DIR__ . '/../../includes/footer.php'; ?>
