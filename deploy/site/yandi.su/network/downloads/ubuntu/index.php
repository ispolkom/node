<?php
$downloadPlatform = 'Ubuntu'; $downloadIcon = '/assets/icons/ubuntu.svg'; $downloadRelease = 'debian';
$downloadStatus = 'Test build available';
$downloadDescription = 'The same two .deb packages as for Debian: the node and the desktop shell. Install both packages of the same version together.';
$downloadSteps = ['Download both files into one folder: <code>yandi-node_*.deb</code> and <code>yandi-desktop_*.deb</code> (below).', 'Optional, check the files: <pre>sha256sum yandi-node_*.deb yandi-desktop_*.deb</pre> and compare with the sums above.', 'Install both packages with one command: <pre>sudo apt install ./yandi-node_*.deb ./yandi-desktop_*.deb</pre>', 'Start the shell from the application menu (YANDI) or with <code>yandi-desktop</code>. On the first start it offers to set a password and create the private environment.', 'To keep the node running as a system service and to open the firewall ports, follow <code>/usr/share/doc/yandi-node/DEPLOYMENT.md</code>.'];
$downloadNotes = ['The packages are built on Debian 12 and need glibc 2.36 or newer: Ubuntu 24.04 and newer. Ubuntu 22.04 is too old.', 'The packages have not been tested on Ubuntu yet.', 'Removing: <code>sudo apt remove yandi-desktop yandi-node</code>.'];
$downloadCli = true;
require __DIR__ . '/../_page.php';
