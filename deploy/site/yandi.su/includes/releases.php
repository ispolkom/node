<?php
declare(strict_types=1);
// Файлы для скачивания лежат в тех же папках update/, из которых узлы и приложение берут обновления (подписанный MANIFEST.json рядом).
const YANDI_RELEASES = [
    'android' => ['version' => '1.0.0 (build 1)',
        'date' => '2026-10-20',
        'changes' => [
            'en' => ['Clean release signed with a new developer key (the previous test builds are not updated to this one: remove them and install this build).', 'Two languages in the app: Russian and English (the system language is used on first start, switchable in the app).', 'New first start: connect with a pairing code from your node, or enter as a guest through a node from the signed list of entry nodes.', 'Invite a person from your own device: an 8-digit code with a PIN (or a link), valid 60 minutes; the administrator confirms the new user.', 'Support page with wallets (USDT/TRX/ETH and others) from the signed release manifest.'],
            'ru' => ['Чистый выпуск, подписанный новым ключом разработчика (прежние тестовые сборки на эту не обновляются: удалите их и установите эту).', 'Два языка в приложении: русский и английский (при первом запуске берётся язык системы, переключается в приложении).', 'Новый первый запуск: подключиться кодом сопряжения от своего узла или войти гостем через узел из подписанного списка входных узлов.', 'Приглашение человека со своего устройства: код из 8 цифр с PIN (или ссылка), действует 60 минут; нового пользователя подтверждает администратор.', 'Страница поддержки с кошельками (USDT/TRX/ETH и другие) из подписанного манифеста выпуска.'],
        ],
        'compat' => [
            'en' => 'Works with nodes 0.2.0+g819. This pair has not yet been tested on devices after the clean release. Install the node and the app from this site, and keep both up to date.',
            'ru' => 'Работает с узлами 0.2.0+g819. После чистого выпуска эта пара на устройствах ещё не проверялась. Ставьте узел и приложение с этого сайта и обновляйте оба.',
        ],
        'files' => [
        ['href' => '/update/android/yandi.apk', 'name' => 'yandi.apk', 'size' => '122.8', 'sha256' => 'c49d8c1aa94c9be9355f29e11db0f7072c9584be65e0814a7ca252bb05ea4ee9'],
    ]],
    'debian' => ['version' => '0.2.0+g819',
        'date' => '2026-10-20',
        'changes' => [
            'en' => ['Clean release signed with a new developer key: remove the previous test packages (purge) and install these two.', 'Two languages: the console, installer and shell speak Russian or English (chosen at install, --lang).', 'Console without the shell: yandi setup and yandi admin (including signing the list of entry nodes with a key that lives only in the private environment).', 'Signed list of entry nodes for phones (guest role) and applications from new nodes; invitations of devices with PIN.', 'Shell: copy the pairing code with one click, who invited a user, confirm the user, entry-list panel.'],
            'ru' => ['Чистый выпуск, подписанный новым ключом разработчика: удалите прежние тестовые пакеты (purge) и поставьте эти два.', 'Два языка: консоль, установщик и оболочка говорят по-русски или по-английски (выбор при установке, --lang).', 'Консоль без оболочки: yandi setup и yandi admin (в том числе подпись списка входных узлов ключом, который живёт только в закрытой среде).', 'Подписанный список входных узлов для телефонов (роль guest) и заявки новых узлов; приглашение устройств с PIN.', 'Оболочка: копирование кода сопряжения одним нажатием, кто пригласил пользователя, подтверждение пользователя, панель списка входных узлов.'],
        ],
        'compat' => [
            'en' => 'Install both packages of the same version together. Works with the Android app 1.0.0 (build 1) of the same release.',
            'ru' => 'Оба пакета одной версии ставятся вместе. Работает с приложением для Android 1.0.0 (сборка 1) того же выпуска.',
        ],
        'files' => [
        ['href' => '/update/deb/yandi-node_0.2.0-g819_amd64.deb', 'name' => 'yandi-node_0.2.0-g819_amd64.deb', 'size' => '6.8', 'sha256' => '59023f779ee69c250b52308d445788a92f9430a55d34cc27d38a3a3edbd22469'],
        ['href' => '/update/deb/yandi-desktop_0.2.0-g819_amd64.deb', 'name' => 'yandi-desktop_0.2.0-g819_amd64.deb', 'size' => '28.9', 'sha256' => 'c1c1c882c5e6ffe9e8e0749b8b964ebbfc2b39b7ebd3d3ca3299b29c2e438d94'],
    ]],
];
