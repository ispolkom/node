<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 5)',
        'date' => '2026-10-27',
        'changes' => [
            'ru' => [
                'Выпуск 1.0.0, сборка 5: номер версии Android согласован с версией проекта.',
                'Защита экрана приложения от снимков и записи; обновления гостевого режима.',
                'Сообщения удалённого пользователя отображаются заглушкой, включая цитаты; срок хранения сообщений — 365 суток.'
            ],
            'en' => [
                'Version 1.0.0, build 5: the Android version now follows the project version.',
                'Screen capture and recording protection; guest mode updates.',
                'Messages from deleted users display a placeholder, including quoted messages; message retention is 365 days.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g886. Используйте пакеты этого выпуска.',
            'en' => 'Built together with node 0.2.0+g886. Use the packages from this release.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '123.2',
                'sha256' => '52e2b43394a16474a5a2fb332d4331a331c4988d87f87d965b7a2cb9c941e391'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g886',
        'date' => '2026-10-27',
        'changes' => [
            'ru' => [
                'Узел и оболочка ПК собраны из одной версии исходников.',
                'Исправления автономного и гостевого режимов; обновления удаления пользователей и хранения сообщений.',
                'Пакеты и манифесты подписаны действующим ключом разработчика; SHA-256 приведены для каждого файла.'
            ],
            'en' => [
                'The node and desktop shell are built from the same source revision.',
                'Autonomous and guest mode fixes; updates to user deletion and message retention.',
                'Packages and manifests use the current developer signing key; SHA-256 is listed for each file.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android-приложение этого выпуска: 1.0.0 (сборка 5).',
            'en' => 'Install both packages of the same version together. Android app for this release: 1.0.0 (build 5).'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g886_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g886_amd64.deb',
                'size' => '29.0',
                'sha256' => 'd3b1f544faa151404f8a5f7f63da5955f4f40820b017b89d29c297f452cdef43'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g886_amd64.deb',
                'name' => 'yandi-node_0.2.0-g886_amd64.deb',
                'size' => '7.0',
                'sha256' => '9aefe134d85ab97700712903b180ebc96f53033bcad920a132505b551fc347ed'
            ]
        ]
    ]
];
