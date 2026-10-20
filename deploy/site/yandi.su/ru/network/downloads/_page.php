<?php
require_once __DIR__ . '/../../../includes/releases.php';
$pageTitle = 'Скачать YANDI для ' . $downloadPlatform . ' — тестовая сборка';
$pageDescription = 'Скачать YANDI для ' . $downloadPlatform . ': инструкция по установке, версия, дата выпуска и контрольные суммы. ' . $downloadStatus . '. Тестовая сборка для поиска ошибок.';
$activeNav = 'network';
require __DIR__ . '/../../includes/header.php';
$rel = $downloadRelease !== null ? (YANDI_RELEASES[$downloadRelease] ?? null) : null;
?>
<section class="hero"><div class="container"><h1><?= e($downloadPlatform) ?></h1><p class="lead"><?= e($downloadStatus) ?></p></div></section>
<section><div class="container"><div class="download-detail">
    <div class="notice"><strong>Тестовая версия.</strong> Данная версия тестовая и служит для проверки и выявления багов. Не доверяйте ей важные данные и критически важную связь, а о найденных ошибках сообщайте.</div>
    <span class="platform-icon-shell"><img class="platform-icon" src="<?= e($downloadIcon) ?>" alt="<?= e($downloadPlatform) ?>"></span>
    <h2>YANDI Network для <?= e($downloadPlatform) ?></h2>
    <p><?= e($downloadDescription) ?></p>
    <?php if ($rel): ?>
        <div class="file-box">
        <?php foreach ($rel['files'] as $f): ?>
            <h3><?= e($f['name']) ?></h3>
            <dl><dt>Версия</dt><dd><?= e($rel['version']) ?></dd><dt>Дата выпуска</dt><dd><?= e($rel['date']) ?></dd><dt>Размер</dt><dd><?= e($f['size']) ?> МБ</dd><dt>SHA-256</dt><dd><code><?= e($f['sha256']) ?></code></dd></dl>
            <a class="btn big" href="<?= e($f['href']) ?>" download>Скачать <?= e($f['name']) ?></a>
        <?php endforeach; ?>
        </div>
    <?php else: ?>
        <p class="muted">Сборки для этой платформы пока нет.</p>
    <?php endif; ?>
    <?php if ($rel): ?>
    <div class="download-steps"><h3>Что нового</h3><ul><?php foreach ($rel['changes']['ru'] as $c): ?><li><?= e($c) ?></li><?php endforeach; ?></ul>
    <h3>Совместимость</h3><p><?= e($rel['compat']['ru']) ?></p></div>
    <?php endif; ?>
    <div class="download-steps"><h3>Установка</h3><ol><?php foreach ($downloadSteps as $step): ?><li><?= $step ?></li><?php endforeach; ?></ol></div>
    <?php if (!empty($downloadCli)) { require_once __DIR__ . '/../../includes/cli_guide.php'; yandi_cli_guide('ru'); } ?>
    <?php if (!empty($downloadNotes)): ?><div class="download-steps"><h3>Примечания</h3><ul><?php foreach ($downloadNotes as $n): ?><li><?= $n ?></li><?php endforeach; ?></ul></div><?php endif; ?>
    <p><a class="btn" href="/ru/network/report/">Сообщить об ошибке</a></p>
    <p><a href="/ru/network/">← Вернуться к загрузкам</a></p>
</div></div></section>
<?php require __DIR__ . '/../../includes/footer.php'; ?>
