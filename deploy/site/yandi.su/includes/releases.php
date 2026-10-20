<?php
declare(strict_types=1);
// Файлы для скачивания лежат в тех же папках update/, из которых узлы и приложение берут обновления (подписанный MANIFEST.json рядом).
const YANDI_RELEASES = [
    'android' => ['version' => '1.0.0 (build 2)',
        'date' => '2026-10-20',
        'changes' => [
            'en' => ['New first screen with three tabs: Guest, QR and Code. The camera is on only on the QR tab. Guest sign-in is one button with the warning on the same tab; the node address and region are no longer shown.', 'Clean release signed with a new developer key (the previous test builds are not updated to this one: remove them and install this build).', 'Two languages in the app: Russian and English (the system language is used on first start, switchable in the app).', 'New first start: connect with a pairing code from your node, or enter as a guest through a node from the signed list of entry nodes.', 'Invite a person from your own device: an 8-digit code with a PIN (or a link), valid 60 minutes; the administrator confirms the new user.', 'Support page with wallets (USDT/TRX/ETH and others) from the signed release manifest.'],
            'ru' => ['Новый первый экран с тремя вкладками: «Гость», «QR» и «Код». Камера включена только на вкладке QR. Вход гостем одной кнопкой, предупреждение на той же вкладке; адрес и регион узла больше не показываются.', 'Чистый выпуск, подписанный новым ключом разработчика (прежние тестовые сборки на эту не обновляются: удалите их и установите эту).', 'Два языка в приложении: русский и английский (при первом запуске берётся язык системы, переключается в приложении).', 'Новый первый запуск: подключиться кодом сопряжения от своего узла или войти гостем через узел из подписанного списка входных узлов.', 'Приглашение человека со своего устройства: код из 8 цифр с PIN (или ссылка), действует 60 минут; нового пользователя подтверждает администратор.', 'Страница поддержки с кошельками (USDT/TRX/ETH и другие) из подписанного манифеста выпуска.'],
        ],
        'compat' => [
            'en' => 'Works with nodes 0.2.0+g829. This pair has not yet been tested on devices after the clean release. Install the node and the app from this site, and keep both up to date.',
            'ru' => 'Работает с узлами 0.2.0+g829. После чистого выпуска эта пара на устройствах ещё не проверялась. Ставьте узел и приложение с этого сайта и обновляйте оба.',
        ],
        'files' => [
        ['href' => '/update/android/yandi.apk', 'name' => 'yandi.apk', 'size' => '122.9', 'sha256' => '290b064bd9dfe66322d70e390b063f16de29a15a63d5067e4c8900e977f88d22'],
    ]],
    'debian' => ['version' => '0.2.0+g829',
        'date' => '2026-10-20',
        'changes' => [
            'en' => ['Clean release signed with a new developer key: remove the previous test packages (purge) and install these two.', 'Two languages: the console, installer and shell speak Russian or English (chosen at install, --lang).', 'Console without the shell: yandi setup and yandi admin (including signing the list of entry nodes with a key that lives only in the private environment).', 'Signed list of entry nodes for phones (guest role) and applications from new nodes; invitations of devices with PIN.', 'The package enables and starts the node service (yandi-node@USER) by itself on first install.', 'Shell: copy the pairing code with one click, who invited a user, confirm the user, entry-list panel.'],
            'ru' => ['Чистый выпуск, подписанный новым ключом разработчика: удалите прежние тестовые пакеты (purge) и поставьте эти два.', 'Два языка: консоль, установщик и оболочка говорят по-русски или по-английски (выбор при установке, --lang).', 'Консоль без оболочки: yandi setup и yandi admin (в том числе подпись списка входных узлов ключом, который живёт только в закрытой среде).', 'Подписанный список входных узлов для телефонов (роль guest) и заявки новых узлов; приглашение устройств с PIN.', 'Пакет сам включает и запускает службу узла (yandi-node@ПОЛЬЗОВАТЕЛЬ) при первой установке.', 'Оболочка: копирование кода сопряжения одним нажатием, кто пригласил пользователя, подтверждение пользователя, панель списка входных узлов.'],
        ],
        'compat' => [
            'en' => 'Install both packages of the same version together. Works with the Android app 1.0.0 (build 2) of the same release.',
            'ru' => 'Оба пакета одной версии ставятся вместе. Работает с приложением для Android 1.0.0 (сборка 2) того же выпуска.',
        ],
        'files' => [
        ['href' => '/update/deb/yandi-node_0.2.0-g829_amd64.deb', 'name' => 'yandi-node_0.2.0-g829_amd64.deb', 'size' => '6.8', 'sha256' => 'cc18a02e0f7a1a3f63a9391c8d16d7fbb5a3e943fdd0f6dcba21093b19fc6808'],
        ['href' => '/update/deb/yandi-desktop_0.2.0-g829_amd64.deb', 'name' => 'yandi-desktop_0.2.0-g829_amd64.deb', 'size' => '28.9', 'sha256' => '179a581fa290d5127b05e3435e711780f6d8143a3110209a2155c9e3bac23005'],
    ]],
];
