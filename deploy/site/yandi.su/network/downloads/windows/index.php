<?php
$downloadPlatform = 'Windows'; $downloadIcon = '/assets/icons/windows.svg'; $downloadRelease = null;
$downloadStatus = 'Experimental x64 test build available';
$downloadDescription = 'The console node was cross-compiled on Debian from source revision 2a94ffe (node 0.2.0+g961). It includes the required DLL and a setup guide. The separate desktop shell is not included.';
$downloadTestFile = [
    'href' => '/update/windows/yandi-node_0.2.0-g961_windows-x64-test.zip',
    'name' => 'yandi-node_0.2.0-g961_windows-x64-test.zip',
    'size' => '9.9',
    'sha256' => 'ee1f065ae09690d4ba83e75dfe0996afe25a821e6f9ea1731a74bb1e8042cf22',
];
$downloadSteps = [
    'Download the ZIP and extract it to a writable folder. Keep <code>libssp-0.dll</code> next to <code>yandi.exe</code>.',
    'Open PowerShell in that folder and run <code>$env:YANDI_CONFIG = Join-Path (Get-Location) "yandi-config.yaml"</code>, then <code>.\yandi.exe</code>.',
    'Open <a href="http://127.0.0.1:9999/">http://127.0.0.1:9999/</a> on the same computer. Full instructions are in <a href="/update/windows/README.txt">README.txt</a>.',
];
$downloadNotes = [
    'This test ZIP is not signed with the developer release key; the SHA-256 above is a checksum, not a signature.',
    'Startup and calls on Windows 11 have not yet been tested on a physical laptop.',
];
require __DIR__ . '/../_page.php';
