<?php
declare(strict_types=1);

const RU_BASE = '/ru';
const SITE_YEAR = '2026';

function e(string $value): string
{
    return htmlspecialchars($value, ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8');
}

function ru_navigation(): array
{
    return [
        ['key' => 'home', 'label' => 'Главная', 'href' => RU_BASE . '/'],
        ['key' => 'network', 'label' => 'Сеть', 'href' => RU_BASE . '/network/'],
        ['key' => 'ai', 'label' => 'ИИ', 'href' => RU_BASE . '/ai/'],
        ['key' => 'core', 'label' => 'Ядро', 'href' => RU_BASE . '/core/'],
        ['key' => 'economy', 'label' => 'Экономика', 'href' => RU_BASE . '/economy/'],
        ['key' => 'research', 'label' => 'Исследования', 'href' => RU_BASE . '/research/'],
        ['key' => 'projects', 'label' => 'Проекты', 'href' => RU_BASE . '/projects/'],
        ['key' => 'about', 'label' => 'О YANDI', 'href' => RU_BASE . '/about/'],
        ['key' => 'report', 'label' => 'Сообщить об ошибке', 'href' => RU_BASE . '/network/report/'],
    ];
}

function english_version_url(): string
{
    $path = parse_url($_SERVER['REQUEST_URI'] ?? '/ru/', PHP_URL_PATH) ?: '/ru/';
    $path = preg_replace('#^/ru#', '', $path) ?: '/';
    return $path === '/' ? '/' : rtrim($path, '/') . '/';
}

const SITE_BASE = 'https://yandi.su';

function page_path_en(): string
{
    $p = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/';
    $p = preg_replace('#index\.php$#', '', $p);
    $p = preg_replace('#^/ru(/|$)#', '/', $p) ?: '/';
    return substr($p, -1) === '/' ? $p : $p . '/';
}

/** Теги для поисковиков и ссылок: канонический адрес, версии языков, Open Graph, значки. */
function seo_head(string $lang, string $title, string $description): void
{
    $en = page_path_en();
    $ru = '/ru' . $en;
    $self = SITE_BASE . ($lang === 'ru' ? $ru : $en);
    $img = SITE_BASE . '/og-image.png';
    $locale = $lang === 'ru' ? 'ru_RU' : 'en_US';
    $other = $lang === 'ru' ? 'en_US' : 'ru_RU';
    echo '<link rel="canonical" href="' . e($self) . '">' . "\n";
    echo '    <link rel="alternate" hreflang="en" href="' . e(SITE_BASE . $en) . '">' . "\n";
    echo '    <link rel="alternate" hreflang="ru" href="' . e(SITE_BASE . $ru) . '">' . "\n";
    echo '    <link rel="alternate" hreflang="x-default" href="' . e(SITE_BASE . $en) . '">' . "\n";
    echo '    <meta property="og:type" content="website">' . "\n";
    echo '    <meta property="og:site_name" content="YANDI">' . "\n";
    echo '    <meta property="og:title" content="' . e($title) . '">' . "\n";
    echo '    <meta property="og:description" content="' . e($description) . '">' . "\n";
    echo '    <meta property="og:url" content="' . e($self) . '">' . "\n";
    echo '    <meta property="og:image" content="' . e($img) . '">' . "\n";
    echo '    <meta property="og:image:width" content="1200">' . "\n";
    echo '    <meta property="og:image:height" content="630">' . "\n";
    echo '    <meta property="og:locale" content="' . $locale . '">' . "\n";
    echo '    <meta property="og:locale:alternate" content="' . $other . '">' . "\n";
    echo '    <meta name="twitter:card" content="summary_large_image">' . "\n";
    echo '    <meta name="theme-color" content="#2a6b2a">' . "\n";
    echo '    <link rel="icon" type="image/png" sizes="32x32" href="/favicon-32.png">' . "\n";
    echo '    <link rel="apple-touch-icon" href="/apple-touch-icon.png">' . "\n";
    if ($en === '/') {
        $ld = [
            '@context' => 'https://schema.org',
            '@graph' => [
                ['@type' => 'Organization', 'name' => 'YANDI', 'url' => SITE_BASE . '/', 'logo' => SITE_BASE . '/logo.png', 'sameAs' => ['https://github.com/ispolkom']],
                ['@type' => 'WebSite', 'name' => 'YANDI', 'url' => SITE_BASE . '/', 'inLanguage' => ['en', 'ru']],
            ],
        ];
        echo '    <script type="application/ld+json">' . json_encode($ld, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE) . '</script>' . "\n";
    }
}
