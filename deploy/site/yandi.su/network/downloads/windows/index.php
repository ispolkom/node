<?php
$downloadPlatform = 'Windows'; $downloadIcon = '/assets/icons/windows.svg'; $downloadRelease = null;
$downloadStatus = 'Experimental x64 test build available';
$downloadDescription = 'The console node was cross-compiled on Debian from source revision 474361e (node 0.2.0+g970). It includes the required DLL and a setup guide. The separate desktop shell is not included.';
$downloadTestFile = [
    'href' => '/update/windows/yandi-node_0.2.0-g970_windows-x64-test.zip',
    'name' => 'yandi-node_0.2.0-g970_windows-x64-test.zip',
    'size' => '9.9',
    'sha256' => 'e754915a8874c6af74c1e8889f40662cab2e8acd873d903a8f87a66d116597e0',
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
