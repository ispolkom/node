<?php
$pageTitle = 'YANDI — Открытые проекты';
$pageDescription = 'Открытая часть экосистемы YANDI и публичные репозитории.';
$activeNav = 'projects';
require __DIR__ . '/../includes/header.php';
?>
<section class="hero"><div class="container">
    <h1>Открытые проекты</h1>
    <p class="lead">Публичные репозитории представляют только часть экосистемы YANDI.</p>
    <p>Некоторые проекты находятся в разработке и пока не опубликованы. Поэтому список GitHub нельзя считать полным каталогом YANDI.</p>
</div></section>
<section><div class="container"><h2>Проекты</h2><div class="grid">
<article class="card"><h3>YANDI Node</h3><p>Узел приватной сети: чаты, файлы, группы, голосовые и видеозвонки, выход в интернет через доверенные узлы. Написан на Rust.</p><p class="download-status">Статус: тестовая версия (0.2.0). Репозиторий: <a href="https://github.com/ispolkom/node" rel="noopener noreferrer">github.com/ispolkom/node</a></p><a class="card-link" href="/ru/network/">Загрузки</a></article>
<article class="card"><h3>Приложения YANDI</h3><p>Приложение для Android и оболочка для компьютера (Debian, Ubuntu), которые работают с вашим узлом. Лежат в том же репозитории, что и узел.</p><p class="download-status">Статус: тестовая версия. Windows, macOS и iOS: пока не собраны.</p><a class="card-link" href="/ru/network/">Загрузки</a></article>
</div></div></section>
<section><div class="container">
    <div class="card">
        <h2>GitHub</h2>
        <p>Открытая часть разработки доступна в организации YANDI:</p>
        <p><a href="https://github.com/ispolkom" rel="noopener noreferrer">github.com/ispolkom →</a></p>
        <p class="muted">Статусы отдельных направлений могут различаться: открытый проект, прототип, исследование или разработка.</p>
    </div>
</div></section>
<?php require __DIR__ . '/../includes/footer.php'; ?>
