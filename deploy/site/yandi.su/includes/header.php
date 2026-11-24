<?php
require_once __DIR__ . '/config.php';
$pageTitle = $pageTitle ?? 'YANDI — You & I';
$pageDescription = $pageDescription ?? 'YANDI is an ecosystem for people, artificial intelligence and decentralized networks.';
$activeNav = $activeNav ?? '';
?><!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title><?= e($pageTitle) ?></title>
    <meta name="description" content="<?= e($pageDescription) ?>">
    <?php seo_head('en', $pageTitle, $pageDescription); ?>
    <link rel="stylesheet" href="/style.css?v=20261018-3">
</head>
<body>
<header class="site-header"><div class="container">
    <a class="logo" href="/" aria-label="YANDI — home"><img src="/logo.png" alt="YANDI" width="48" height="48"></a>
    <nav aria-label="Main navigation">
        <?php foreach (en_navigation() as $item): ?>
            <a href="<?= e($item['href']) ?>"<?= $activeNav === $item['key'] ? ' aria-current="page"' : '' ?>><?= e($item['label']) ?></a>
        <?php endforeach; ?>
            <a href="<?= e(russian_version_url()) ?>">Русский</a>
        </nav>
</div></header>
<main>
