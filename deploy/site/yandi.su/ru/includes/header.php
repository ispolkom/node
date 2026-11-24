<?php
require_once __DIR__ . '/config.php';

$pageTitle = $pageTitle ?? 'YANDI — Ты и Я';
$pageDescription = $pageDescription ?? 'YANDI — экосистема для человека, искусственного интеллекта и децентрализованной сети.';
$activeNav = $activeNav ?? '';
?><!doctype html>
<html lang="ru">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title><?= e($pageTitle) ?></title>
    <meta name="description" content="<?= e($pageDescription) ?>">
    <?php seo_head('ru', $pageTitle, $pageDescription); ?>
    <link rel="stylesheet" href="/style.css?v=20261018-3">
</head>
<body>
<header class="site-header">
    <div class="container">
        <a class="logo" href="<?= RU_BASE ?>/" aria-label="YANDI — главная">
            <img src="/logo.png" alt="YANDI" width="48" height="48">
        </a>
        <nav aria-label="Основная навигация">
            <?php foreach (ru_navigation() as $item): ?>
                <a href="<?= e($item['href']) ?>"<?= $activeNav === $item['key'] ? ' aria-current="page"' : '' ?>><?= e($item['label']) ?></a>
            <?php endforeach; ?>
            <a href="<?= e(english_version_url()) ?>">English</a>
        </nav>
    </div>
</header>
<main>
