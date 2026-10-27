<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 6)',
        'date' => '2026-10-27',
        'changes' => [
            'ru' => [
                'Звуки включены по умолчанию: личные сообщения — icq.mp3, группы — icq-online.mp3, аудиозвонки — calling.mp3, видеозвонки — sqype.mp3, дозвон — call_out.mp3; в настройках каждый вид отключается отдельно.',
                'Выпуск 1.0.0, сборка 6: номер версии Android согласован с версией проекта.',
                'Защита экрана приложения от снимков и записи; обновления гостевого режима.',
                'Сообщения удалённого пользователя отображаются заглушкой, включая цитаты; срок хранения сообщений — 365 суток.'
            ],
            'en' => [
                'Default-on YANDI tones: private messages, group events, audio calls, video calls and outgoing calls; each category can be disabled independently.',
                'Version 1.0.0, build 6: the Android version now follows the project version.',
                'Screen capture and recording protection; guest mode updates.',
                'Messages from deleted users display a placeholder, including quoted messages; message retention is 365 days.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g889. Используйте пакеты этого выпуска.',
            'en' => 'Built together with node 0.2.0+g889. Use the packages from this release.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '123.2',
                'sha256' => '3d17818cafcfa94fb3a59c393fdddb235d5a5a570d286e82020be0b4b5a1c61a'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g889',
        'date' => '2026-10-27',
        'changes' => [
            'ru' => [
                'Пять оригинальных звуков YANDI включены по умолчанию; верхний колокольчик отключает и включает все звуки. Исправлены запуск звука видео и права ярлыка и значков для меню приложений.',
                'Узел и оболочка ПК собраны из одной версии исходников.',
                'Исправления автономного и гостевого режимов; обновления удаления пользователей и хранения сообщений.',
                'Пакеты и манифесты подписаны действующим ключом разработчика; SHA-256 приведены для каждого файла.'
            ],
            'en' => [
                'Five default-on YANDI tones and a master bell; video audio initialization and launcher/icon permissions fixed.',
                'The node and desktop shell are built from the same source revision.',
                'Autonomous and guest mode fixes; updates to user deletion and message retention.',
                'Packages and manifests use the current developer signing key; SHA-256 is listed for each file.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android-приложение этого выпуска: 1.0.0 (сборка 6).',
            'en' => 'Install both packages of the same version together. Android app for this release: 1.0.0 (build 6).'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g889_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g889_amd64.deb',
                'size' => '30.7',
                'sha256' => '35424ecbbefa59d3a8fff97dccf54da5e681c580789f23b6778cdaf79b843a29'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g889_amd64.deb',
                'name' => 'yandi-node_0.2.0-g889_amd64.deb',
                'size' => '7.0',
                'sha256' => '19ba976bc2ba7d167c14966a01cb2745949f1401906b2837165bee739ebd4e20'
            ]
        ]
    ]
];
