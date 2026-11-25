<?php
$downloadPlatform = 'Windows'; $downloadIcon = '/assets/icons/windows.svg'; $downloadRelease = null;
$downloadStatus = 'Экспериментальная сборка x64 доступна';
$downloadDescription = 'Консольная нода собрана для Windows на Debian из коммита 474361e (узел 0.2.0+g970). В архиве есть необходимая DLL и инструкция. Отдельная оболочка ПК в комплект не входит.';
$downloadTestFile = [
    'href' => '/update/windows/yandi-node_0.2.0-g970_windows-x64-test.zip',
    'name' => 'yandi-node_0.2.0-g970_windows-x64-test.zip',
    'size' => '9,9',
    'sha256' => 'e754915a8874c6af74c1e8889f40662cab2e8acd873d903a8f87a66d116597e0',
];
$downloadSteps = [
    'Скачайте ZIP и распакуйте в папку с правом записи. Держите <code>libssp-0.dll</code> рядом с <code>yandi.exe</code>.',
    'Откройте PowerShell в этой папке и выполните <code>$env:YANDI_CONFIG = Join-Path (Get-Location) "yandi-config.yaml"</code>, затем <code>.\yandi.exe</code>.',
    'Откройте на этом же ПК <a href="http://127.0.0.1:9999/">http://127.0.0.1:9999/</a>. Подробности — в <a href="/update/windows/README.txt">README.txt</a>.',
];
$downloadNotes = [
    'Тестовый ZIP не подписан ключом разработчика; SHA-256 выше — контрольная сумма, а не подпись.',
    'Запуск и звонки на ноутбуке с Windows 11 пока не проверены.',
];
require __DIR__ . '/../_page.php';
