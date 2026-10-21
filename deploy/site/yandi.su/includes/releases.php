<?php
declare(strict_types=1);
// Файлы для скачивания лежат в тех же папках update/, из которых узлы и приложение берут обновления (подписанный MANIFEST.json рядом).
const YANDI_RELEASES = [
    'android' => ['version' => '1.0.0 (build 4)',
        'date' => '2026-10-20',
        'changes' => [
            'en' => ['Search page: three cards (Guests, Users, Groups) with online status and avatars; a guest sees only people who are online; people not seen for a day are hidden until they return; on the PC, Delete contact returns the person to the users list instead of hiding them; dependency security updates.', 'Guest access is on by default and a guest is a real chat partner: sees the node users, writes and makes audio calls (15 minutes each, 60 minutes a day); closed features (groups, VPN, video, invitations) stay visible and open a yellow notice; a guest can ask the administrator for user status with a message.', 'New first screen with three tabs: Guest, QR and Code. The camera is on only on the QR tab. Guest sign-in is one button with the warning on the same tab; the node address and region are no longer shown.', 'Clean release signed with a new developer key (the previous test builds are not updated to this one: remove them and install this build).', 'Two languages in the app: Russian and English (the system language is used on first start, switchable in the app).', 'New first start: connect with a pairing code from your node, or enter as a guest through a node from the signed list of entry nodes.', 'Invite a person from your own device: an 8-digit code with a PIN (or a link), valid 60 minutes; the administrator confirms the new user.', 'Support page with wallets (USDT/TRX/ETH and others) from the signed release manifest.'],
            'ru' => ['Страница поиска: три карточки (Гости, Пользователи, Группы) со статусом и аватарами; гость видит только тех, кто в сети; не бывшие в сети больше суток прячутся до возвращения; на ПК «Удалить контакт» возвращает человека в список пользователей, а не скрывает; обновления зависимостей по безопасности.', 'Гости принимаются по умолчанию, гость стал настоящим собеседником: видит пользователей узла, пишет и звонит по аудио (15 минут на звонок, 60 минут в сутки); закрытое (группы, VPN, видео, приглашения) остаётся на виду и открывает жёлтое окно; гость может запросить у администратора статус пользователя с текстом.', 'Новый первый экран с тремя вкладками: «Гость», «QR» и «Код». Камера включена только на вкладке QR. Вход гостем одной кнопкой, предупреждение на той же вкладке; адрес и регион узла больше не показываются.', 'Чистый выпуск, подписанный новым ключом разработчика (прежние тестовые сборки на эту не обновляются: удалите их и установите эту).', 'Два языка в приложении: русский и английский (при первом запуске берётся язык системы, переключается в приложении).', 'Новый первый запуск: подключиться кодом сопряжения от своего узла или войти гостем через узел из подписанного списка входных узлов.', 'Приглашение человека со своего устройства: код из 8 цифр с PIN (или ссылка), действует 60 минут; нового пользователя подтверждает администратор.', 'Страница поддержки с кошельками (USDT/TRX/ETH и другие) из подписанного манифеста выпуска.'],
        ],
        'compat' => [
            'en' => 'Works with nodes 0.2.0+g843. This pair has not yet been tested on devices after the clean release. Install the node and the app from this site, and keep both up to date.',
            'ru' => 'Работает с узлами 0.2.0+g843. После чистого выпуска эта пара на устройствах ещё не проверялась. Ставьте узел и приложение с этого сайта и обновляйте оба.',
        ],
        'files' => [
        ['href' => '/update/android/yandi.apk', 'name' => 'yandi.apk', 'size' => '123.0', 'sha256' => '592ebe09bac0f458374f94244f5b4d805b39dcd918595bc78984546e22b45871'],
    ]],
    'debian' => ['version' => '0.2.0+g843',
        'date' => '2026-10-20',
        'changes' => [
            'en' => ['Clean release signed with a new developer key: remove the previous test packages (purge) and install these two.', 'Two languages: the console, installer and shell speak Russian or English (chosen at install, --lang).', 'Console without the shell: yandi setup and yandi admin (including signing the list of entry nodes with a key that lives only in the private environment).', 'Signed list of entry nodes for phones (guest role) and applications from new nodes; invitations of devices with PIN.', 'The package enables and starts the node service (yandi-node@USER) by itself on first install.', 'Shell: copy the pairing code with one click, who invited a user, confirm the user, entry-list panel.'],
            'ru' => ['Чистый выпуск, подписанный новым ключом разработчика: удалите прежние тестовые пакеты (purge) и поставьте эти два.', 'Два языка: консоль, установщик и оболочка говорят по-русски или по-английски (выбор при установке, --lang).', 'Консоль без оболочки: yandi setup и yandi admin (в том числе подпись списка входных узлов ключом, который живёт только в закрытой среде).', 'Подписанный список входных узлов для телефонов (роль guest) и заявки новых узлов; приглашение устройств с PIN.', 'Пакет сам включает и запускает службу узла (yandi-node@ПОЛЬЗОВАТЕЛЬ) при первой установке.', 'Оболочка: копирование кода сопряжения одним нажатием, кто пригласил пользователя, подтверждение пользователя, панель списка входных узлов.'],
        ],
        'compat' => [
            'en' => 'Install both packages of the same version together. Works with the Android app 1.0.0 (build 4) of the same release.',
            'ru' => 'Оба пакета одной версии ставятся вместе. Работает с приложением для Android 1.0.0 (сборка 2) того же выпуска.',
        ],
        'files' => [
        ['href' => '/update/deb/yandi-node_0.2.0-g843_amd64.deb', 'name' => 'yandi-node_0.2.0-g843_amd64.deb', 'size' => '6.8', 'sha256' => 'ec97a6e525ff47b3407db8334fa0b6790aea83f416bd7aeb0ba90413e819cd0e'],
        ['href' => '/update/deb/yandi-desktop_0.2.0-g843_amd64.deb', 'name' => 'yandi-desktop_0.2.0-g843_amd64.deb', 'size' => '28.9', 'sha256' => '9f2c5c6e5744b742f6e1b1bac10a96a2fb9f1716f50a09178ffa82d727a1aa11'],
    ]],
];
