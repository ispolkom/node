<?php
// Общий блок «Сервер без оболочки (консоль)» для страниц Debian и Ubuntu (русский и английский). Правится в одном месте.
function yandi_cli_guide(string $lang): void {
    $ru = $lang === 'ru';
    $unit = <<<'UNIT'
sudo tee /etc/systemd/system/yandi-node.service >/dev/null <<EOF
[Unit]
Description=YANDI node
After=network-online.target
Wants=network-online.target

[Service]
User=$USER
Environment=YANDI_MOBILE_TLS_PORT=8444
ExecStartPre=/usr/bin/mkdir -p -m 0700 $HOME/.local/share/yandi
WorkingDirectory=$HOME/.local/share/yandi
ExecStart=/usr/bin/yandi
Restart=on-failure
RestartSec=5
LimitNOFILE=65536
NoNewPrivileges=yes
PrivateTmp=yes

[Install]
WantedBy=multi-user.target
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now yandi-node
systemctl status yandi-node
UNIT;
    $t = $ru ? [
        'h' => 'Сервер без оболочки (только консоль)',
        'lead' => 'Оболочка (окно) не обязательна: узел полностью настраивается и управляется из терминала. Так обычно ставят узел на сервер.',
        's1' => 'Установка только узла', 'p1' => 'Скачайте только <code>yandi-node_*.deb</code> (пакет оболочки <code>yandi-desktop</code> нужен лишь для окна управления) и установите:',
        's2' => 'Первая настройка', 'p2' => 'Запускайте от обычного пользователя (не от root): данные узла лежат в его домашней папке. Мастер спросит язык, задаст пароль владельца, покажет сид-фразу (12 слов; <code>--words 24</code> для 24) и создаст закрытую среду:',
        'w2' => '<strong>Запишите сид-фразу на бумагу и проверьте.</strong> Она показывается один раз и нужна для восстановления; без неё и пароля данные не вернуть. Без терминала: <code>yandi setup --seed-file seed.txt --password-stdin</code> (фраза пишется в новый файл с правами 0600).',
        's3' => 'Служба и порты', 'p3' => 'Чтобы узел работал сам и поднимался после перезагрузки, создайте службу systemd для вашего пользователя:',
        'p3b' => 'Откройте порты (пример для <code>ufw</code>; для <code>nftables</code> см. <code>/usr/share/doc/yandi-node/DEPLOYMENT.md</code>): 8444/TCP — вход для приложения на телефоне; 9000/UDP и 10000/UDP — сеть узлов. Рекомендуем и увеличить буфер UDP:',
        's4' => 'Управление из консоли', 'p4' => 'Команда <code>yandi admin</code> спрашивает пароль администратора (сеанс 15 минут). Без аргументов открывает меню; основные команды:',
        'c4' => ['yandi admin status' => 'состояние узла', 'yandi admin devices' => 'устройства и пользователи', 'yandi admin pairing owner --host ВАШ_АДРЕС' => 'код и QR для сопряжения телефона', 'yandi admin invites show' => 'политика приглашений', 'yandi admin device-promote ID' => 'подтвердить приглашённого пользователя', 'yandi admin device-remove ID' => 'отвязать устройство', 'yandi admin vault status' => 'закрытая среда: замок, смена пароля', 'yandi admin bootstrap applications' => 'заявки новых узлов (для владельца списка входных узлов)'],
        's5' => 'Подключение телефона', 'p5' => 'Выполните <code>yandi admin pairing owner --host ВАШ_ВНЕШНИЙ_АДРЕС</code>: узел покажет код сопряжения и QR. В приложении YANDI на первом экране наведите камеру на QR или вставьте текст кода (он начинается с <code>YANDI-PAIR-1:</code>). Код одноразовый.',
        's6' => 'Язык', 'p6' => 'Язык консоли: ключ <code>--lang ru|en</code>, переменная <code>YANDI_LANG=en</code> или сохранённый выбор (<code>~/.config/yandi/lang</code>). Без выбора берётся язык системы.',
        's7' => 'Журнал, обновление, удаление',
        'l7' => ['Журнал службы: <code>journalctl -u yandi-node -f</code>.', 'Обновление: установите новый пакет поверх старого (<code>sudo apt install ./yandi-node_*.deb</code>), затем <code>sudo systemctl restart yandi-node</code>. Данные сохраняются.', 'Удаление программы с сохранением данных: <code>sudo apt remove yandi-node</code>. Полное удаление вместе с данными (затирает и удаляет без следа): <code>sudo apt purge yandi-node</code>; перед этим убедитесь, что сид-фраза записана.'],
    ] : [
        'h' => 'Server without the shell (console only)',
        'lead' => 'The shell (the window) is optional: the node can be fully set up and managed from a terminal. This is how a node is usually installed on a server.',
        's1' => 'Install the node only', 'p1' => 'Download only <code>yandi-node_*.deb</code> (the <code>yandi-desktop</code> package is needed just for the management window) and install it:',
        's2' => 'First setup', 'p2' => 'Run it as a regular user (not root): the node data lives in that user\'s home directory. The wizard asks for the language, sets the owner password, shows the seed phrase (12 words; <code>--words 24</code> for 24) and creates the private environment:',
        'w2' => '<strong>Write the seed phrase on paper and check it.</strong> It is shown once and is needed for recovery; without it and the password the data cannot be restored. Without a terminal: <code>yandi setup --seed-file seed.txt --password-stdin</code> (the phrase goes to a new file with mode 0600).',
        's3' => 'Service and ports', 'p3' => 'To keep the node running and bring it up after a reboot, create a systemd service for your user:',
        'p3b' => 'Open the ports (an example for <code>ufw</code>; for <code>nftables</code> see <code>/usr/share/doc/yandi-node/DEPLOYMENT.md</code>): 8444/TCP — entrance for the phone app; 9000/UDP and 10000/UDP — the node network. We also recommend a bigger UDP buffer:',
        's4' => 'Managing from the console', 'p4' => '<code>yandi admin</code> asks for the administrator password (a 15-minute session). Without arguments it opens a menu; the main commands:',
        'c4' => ['yandi admin status' => 'node status', 'yandi admin devices' => 'devices and users', 'yandi admin pairing owner --host YOUR_ADDRESS' => 'code and QR to pair a phone', 'yandi admin invites show' => 'invitation policy', 'yandi admin device-promote ID' => 'confirm an invited user', 'yandi admin device-remove ID' => 'unpair a device', 'yandi admin vault status' => 'private environment: lock, change password', 'yandi admin bootstrap applications' => 'applications from new nodes (for the owner of the entry-node list)'],
        's5' => 'Connecting a phone', 'p5' => 'Run <code>yandi admin pairing owner --host YOUR_PUBLIC_ADDRESS</code>: the node prints a pairing code and a QR. On the first screen of the YANDI app point the camera at the QR or paste the code text (it starts with <code>YANDI-PAIR-1:</code>). The code is single-use.',
        's6' => 'Language', 'p6' => 'Console language: the <code>--lang ru|en</code> option, the <code>YANDI_LANG=en</code> variable or the saved choice (<code>~/.config/yandi/lang</code>). Without a choice the system language is used.',
        's7' => 'Log, updating, removal',
        'l7' => ['Service log: <code>journalctl -u yandi-node -f</code>.', 'Updating: install the new package over the old one (<code>sudo apt install ./yandi-node_*.deb</code>), then <code>sudo systemctl restart yandi-node</code>. The data is kept.', 'Removing the program but keeping the data: <code>sudo apt remove yandi-node</code>. Complete removal together with the data (wiped without a trace): <code>sudo apt purge yandi-node</code>; make sure the seed phrase is written down first.'],
    ];
    $sysctl = "sudo ufw allow 8444/tcp\nsudo ufw allow 9000/udp\nsudo ufw allow 10000/udp\n\necho -e 'net.core.rmem_max=67108864\\nnet.core.wmem_max=67108864' | sudo tee /etc/sysctl.d/99-yandi.conf\nsudo sysctl --system";
    ?>
    <div class="download-steps"><h3><?= $t['h'] ?></h3><p><?= $t['lead'] ?></p>
    <h4>1. <?= $t['s1'] ?></h4><p><?= $t['p1'] ?></p><pre>sudo apt install ./yandi-node_*.deb</pre>
    <h4>2. <?= $t['s2'] ?></h4><p><?= $t['p2'] ?></p><pre>yandi setup</pre><p><?= $t['w2'] ?></p>
    <h4>3. <?= $t['s3'] ?></h4><p><?= $t['p3'] ?></p><pre><?= htmlspecialchars($unit, ENT_QUOTES) ?></pre><p><?= $t['p3b'] ?></p><pre><?= htmlspecialchars($sysctl, ENT_QUOTES) ?></pre>
    <h4>4. <?= $t['s4'] ?></h4><p><?= $t['p4'] ?></p><ul><?php foreach ($t['c4'] as $cmd => $what): ?><li><code><?= htmlspecialchars($cmd, ENT_QUOTES) ?></code> — <?= $what ?></li><?php endforeach; ?></ul>
    <h4>5. <?= $t['s5'] ?></h4><p><?= $t['p5'] ?></p>
    <h4>6. <?= $t['s6'] ?></h4><p><?= $t['p6'] ?></p>
    <h4>7. <?= $t['s7'] ?></h4><ul><?php foreach ($t['l7'] as $l): ?><li><?= $l ?></li><?php endforeach; ?></ul></div>
    <?php
}
