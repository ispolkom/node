<?php
$downloadPlatform = 'Windows'; $downloadIcon = '/assets/icons/windows.svg'; $downloadRelease = null;
$downloadStatus = 'Экспериментальная сборка x64 доступна';
$downloadDescription = 'Консольная нода собрана для Windows на Debian из коммита 2a94ffe (узел 0.2.0+g961). В архиве есть необходимая DLL и инструкция. Отдельная оболочка ПК в комплект не входит.';
$downloadTestFile = [
    'href' => '/update/windows/yandi-node_0.2.0-g961_windows-x64-test.zip',
    'name' => 'yandi-node_0.2.0-g961_windows-x64-test.zip',
    'size' => '9,9',
    'sha256' => 'ee1f065ae09690d4ba83e75dfe0996afe25a821e6f9ea1731a74bb1e8042cf22',
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
