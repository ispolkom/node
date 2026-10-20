<?php
declare(strict_types=1);

const SITE_YEAR = '2026';

function e(string $value): string
{
    return htmlspecialchars($value, ENT_QUOTES | ENT_SUBSTITUTE, 'UTF-8');
}

function en_navigation(): array
{
    return [
        ['key' => 'home', 'label' => 'Home', 'href' => '/'],
        ['key' => 'network', 'label' => 'Network', 'href' => '/network/'],
        ['key' => 'ai', 'label' => 'AI', 'href' => '/ai/'],
        ['key' => 'core', 'label' => 'Core', 'href' => '/core/'],
        ['key' => 'economy', 'label' => 'Economy', 'href' => '/economy/'],
        ['key' => 'research', 'label' => 'Research', 'href' => '/research/'],
        ['key' => 'projects', 'label' => 'Projects', 'href' => '/projects/'],
        ['key' => 'about', 'label' => 'About YANDI', 'href' => '/about/'],
        ['key' => 'report', 'label' => 'Report a bug', 'href' => '/network/report/'],
    ];
}

function russian_version_url(): string
{
    $path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/';
    return $path === '/' ? '/ru/' : '/ru' . rtrim($path, '/') . '/';
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
