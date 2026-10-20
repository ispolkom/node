<?php
$downloadPlatform = 'Debian'; $downloadIcon = '/assets/icons/debian.svg'; $downloadRelease = 'debian';
$downloadStatus = 'Тестовая сборка доступна';
$downloadDescription = 'Узел (фоновая программа с сетевым слоем) и оболочка для компьютера (окно управления). Устанавливайте оба пакета одной версии вместе.';
$downloadSteps = ['Скачайте оба файла в одну папку: <code>yandi-node_*.deb</code> и <code>yandi-desktop_*.deb</code>.', 'По желанию проверьте файлы: <pre>sha256sum yandi-node_*.deb yandi-desktop_*.deb</pre> и сравните с суммами выше.', 'Установите оба пакета одной командой (APT сам подтянет зависимости): <pre>sudo apt install ./yandi-node_*.deb ./yandi-desktop_*.deb</pre>', 'Запустите оболочку: значок YANDI в меню «Интернет» или команда <code>yandi-desktop</code>. При первом запуске она предложит задать пароль и создать закрытую среду.', 'Чтобы узел работал как системная служба и были открыты порты брандмауэра, следуйте руководству, которое ставится с пакетом: <code>/usr/share/doc/yandi-node/DEPLOYMENT.md</code>.'];
$downloadNotes = ['Нужен Debian 12 (bookworm) или новее, 64 бита (amd64): glibc 2.36 или новее.', 'Обновление: установите новые файлы так же, поверх старых. Данные сохраняются.', 'Удаление: <code>sudo apt remove yandi-desktop yandi-node</code>. Данные остаются в каталоге пользователя, пока вы сами их не удалите.'];
$downloadCli = true;
require __DIR__ . '/../_page.php';
