<?php
$pageTitle = 'YANDI Network — приватная P2P-сеть, скачать для Android и Linux';
$pageDescription = 'YANDI Network — приватная P2P-сеть из своих и доверенных узлов: защищённые чаты, файлы и звонки. Скачайте тестовую сборку для Android, Debian и Ubuntu.';
$activeNav = 'network';
require __DIR__ . '/../includes/header.php';
?>
<section class="hero"><div class="container">
    <h1>YANDI Network</h1>
    <p class="lead">Приватная сеть сообщества, которую создают сами её участники.</p>
    <p>YANDI Network — одно из направлений экосистемы YANDI, а не вся YANDI. Участники могут разворачивать собственные ноды и устанавливать соединения между доверенными устройствами.</p>
</div></section>
<section class="downloads-section"><div class="container">
    <h2>Загрузки</h2><div class="notice"><strong>Тестовая версия.</strong> Данная версия тестовая и служит для проверки и выявления багов. Не доверяйте ей важные данные и критически важную связь, а о найденных ошибках сообщайте.</div>
    <p>Загрузки клиента YANDI и материалы для платформ. Тестовые сборки есть для Android, Debian и Ubuntu; для остальных платформ сборок пока нет.</p>
    <div class="download-grid">
<article class="card download-card is-ready"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/android.svg" alt="Android"></span><div><h3>Android</h3><p class="download-status">Тестовая сборка доступна</p></div><a class="card-link" href="/ru/network/downloads/android/">Подробнее</a></article>
<article class="card download-card is-ready"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/debian.svg" alt="Debian"></span><div><h3>Debian</h3><p class="download-status">Тестовая сборка доступна</p></div><a class="card-link" href="/ru/network/downloads/debian/">Подробнее</a></article>
<article class="card download-card is-ready"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/ubuntu.svg" alt="Ubuntu"></span><div><h3>Ubuntu</h3><p class="download-status">Тестовая сборка доступна</p></div><a class="card-link" href="/ru/network/downloads/ubuntu/">Подробнее</a></article>
<article class="card download-card"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/windows.svg" alt="Windows"></span><div><h3>Windows</h3><p class="download-status">Пока недоступно</p></div><a class="card-link" href="/ru/network/downloads/windows/">Подробнее</a></article>
<article class="card download-card"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/macos.svg" alt="macOS"></span><div><h3>macOS</h3><p class="download-status">Пока недоступно</p></div><a class="card-link" href="/ru/network/downloads/macos/">Подробнее</a></article>
<article class="card download-card"><span class="platform-icon-shell"><img class="platform-icon" src="/assets/icons/apple.svg" alt="iPhone / iPad"></span><div><h3>iPhone / iPad</h3><p class="download-status">Пока недоступно</p></div><a class="card-link" href="/ru/network/downloads/apple/">Подробнее</a></article>
</div><p><a class="btn" href="/ru/network/report/">Сообщить об ошибке</a></p></div></section>
<section><div class="container">
    <h2>Принципы сети</h2>
    <div class="grid">
        <article class="card"><h3>Свои ноды</h3><p>Ноды принадлежат участникам. Пользователь сам выбирает устройства и узлы, которым доверяет.</p></article>
        <article class="card"><h3>Прямая коммуникация</h3><p>P2P-соединения, приватные сообщения, передача файлов, аудиозвонки и видеосвязь.</p></article>
        <article class="card"><h3>Приватность</h3><p>E2E-коммуникация, приглашения для добавления участников и отсутствие обязательного публичного каталога пользователей.</p></article>
        <article class="card"><h3>Без единого обязательного сервера</h3><p>Нет единственного сервера YANDI, которому каждый обязан доверять. Архитектура допускает собственный круг доверия и несколько узлов.</p></article>
    </div>
</div></section>
<section><div class="container">
    <h2>Мобильный доступ</h2>
    <p>Телефон не обязан быть полноценной постоянно доступной P2P-нодой. Он может подключаться к выбранной доверенной ноде пользователя или сообщества:</p>
    <p><strong>Телефон → доверенная нода → другие ноды сообщества</strong></p>
    <p>Это не превращает доверенную ноду в обязательный центральный сервер YANDI: пользователь сам выбирает узел, а в перспективе может использовать несколько доверенных узлов для отказоустойчивости.</p>
</div></section>
<?php require __DIR__ . '/../includes/footer.php'; ?>
