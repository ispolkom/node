<?php
$downloadPlatform = 'Ubuntu'; $downloadIcon = '/assets/icons/ubuntu.svg'; $downloadRelease = 'debian';
$downloadStatus = 'Тестовая сборка доступна';
$downloadDescription = 'Те же два пакета .deb, что и для Debian: узел и оболочка для компьютера. Устанавливайте оба пакета одной версии вместе.';
$downloadSteps = ['Скачайте оба файла в одну папку: <code>yandi-node_*.deb</code> и <code>yandi-desktop_*.deb</code> (ниже).', 'По желанию проверьте файлы: <pre>sha256sum yandi-node_*.deb yandi-desktop_*.deb</pre> и сравните с суммами выше.', 'Установите оба пакета одной командой: <pre>sudo apt install ./yandi-node_*.deb ./yandi-desktop_*.deb</pre>', 'Запустите оболочку из меню приложений (YANDI) или командой <code>yandi-desktop</code>. При первом запуске она предложит задать пароль и создать закрытую среду.', 'Чтобы узел работал как системная служба и были открыты порты брандмауэра, следуйте <code>/usr/share/doc/yandi-node/DEPLOYMENT.md</code>.'];
$downloadNotes = ['Пакеты собраны на Debian 12 и требуют glibc 2.36 или новее: Ubuntu 24.04 и новее. Ubuntu 22.04 слишком старая.', 'На Ubuntu пакеты пока не проверялись.', 'Удаление: <code>sudo apt remove yandi-desktop yandi-node</code>.'];
$downloadCli = true;
require __DIR__ . '/../_page.php';
