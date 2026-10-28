<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 8)',
        'date' => '2026-10-28',
        'changes' => [
            'ru' => [
                'Звуки личных сообщений и фоновые оповещения icq.mp3; значок уведомлений сохранён после R8.',
                'Контакты и локальная история доступны без сети после разблокировки.'
            ],
            'en' => [
                'Private message sounds and bundled icq.mp3 background alerts; notification icon retained through R8.',
                'Contacts and local history remain available offline after unlocking.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g895.',
            'en' => 'Built together with node 0.2.0+g895.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '123.3',
                'sha256' => 'b079c4c6675bbc4f9c4139feb4cb32650a37fc44eb331243c7a8059f23939784'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g895',
        'date' => '2026-10-28',
        'changes' => [
            'ru' => [
                'Панель аудиозвонка, видеозвонка, файлов и очистки закреплена над историей; переписка прокручивается отдельно в широком и узком окне.',
                'Исправлена потеря обработчиков исходящего звонка при переходе в чат. Очистка переписки для звонка не требуется.',
                'Звонящие и текущие вызовы сохраняются при потере фокуса окна.'
            ],
            'en' => [
                'The audio/video call, files and clear-chat toolbar stays above the independently scrolling history in wide and narrow windows.',
                'Fixed outgoing call handlers being detached during chat navigation; clearing history is unnecessary.',
                'Ringing and ongoing calls survive focus changes.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android: 1.0.0, сборка 8.',
            'en' => 'Install both packages of the same version together. Android: 1.0.0 build 8.'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g895_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g895_amd64.deb',
                'size' => '30.7',
                'sha256' => '3267f78350e732a2ee987a927adb1eb6478e9f0b30e803095f66db33f6f27422'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g895_amd64.deb',
                'name' => 'yandi-node_0.2.0-g895_amd64.deb',
                'size' => '7.0',
                'sha256' => '886355b32ba87b7ef1e41e3f07e8e5538e1ce0560667adae131d97a017b73a07'
            ]
        ]
    ]
];
