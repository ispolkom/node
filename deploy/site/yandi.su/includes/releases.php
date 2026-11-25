<?php
declare(strict_types=1);
// Download metadata verified against signed release manifests.
const YANDI_RELEASES = [
    'android' => [
        'version' => '1.0.0 (build 10)',
        'date' => '2026-11-25',
        'changes' => [
            'ru' => [
                'Исправлена фоновая доставка сигналов входящих звонков с подтверждением после E2E-обработки.',
                'Повторные приглашения и несколько одновременных вызовов обрабатываются с сохранением состояния звонка.'
            ],
            'en' => [
                'Background incoming-call signals are acknowledged after E2E processing.',
                'Repeated invitations and concurrent calls preserve call state.'
            ]
        ],
        'compat' => [
            'ru' => 'Собрано вместе с узлом 0.2.0+g970.',
            'en' => 'Built together with node 0.2.0+g970.'
        ],
        'files' => [
            [
                'href' => '/update/android/yandi.apk',
                'name' => 'yandi.apk',
                'size' => '130.6',
                'sha256' => '2ba442f52c0760b3da7f16dcb5b398c065685f8746ca784df9316bde17eb600d'
            ]
        ]
    ],
    'debian' => [
        'version' => '0.2.0+g970',
        'date' => '2026-11-25',
        'changes' => [
            'ru' => [
                'Оболочка продолжает проверять звонки, когда её окно находится в фоне.',
                'Исправлены зависимости пакетов и запуск пользовательской службы после установки и обновления.',
                'Закрыты ошибки хранения очереди сообщений и блокировки личной среды при сбоях записи и смене ключей.'
            ],
            'en' => [
                'The desktop shell continues polling for calls while its window is in the background.',
                'Package dependencies and user service startup after install and upgrade were corrected.',
                'Fixed message queue preservation and private environment lock behavior during failed writes and key changes.'
            ]
        ],
        'compat' => [
            'ru' => 'Установите оба пакета одной версии вместе. Android: 1.0.0, сборка 10.',
            'en' => 'Install both packages of the same version together. Android: 1.0.0 build 10.'
        ],
        'files' => [
            [
                'href' => '/update/deb/yandi-desktop_0.2.0-g970_amd64.deb',
                'name' => 'yandi-desktop_0.2.0-g970_amd64.deb',
                'size' => '32.2',
                'sha256' => '3bfa2abae46c7d28ad2a69fabc7554d73ea83dbe3f619d960d5fb4ca65f8f950'
            ],
            [
                'href' => '/update/deb/yandi-node_0.2.0-g970_amd64.deb',
                'name' => 'yandi-node_0.2.0-g970_amd64.deb',
                'size' => '7.3',
                'sha256' => '18423175f6a4e7939fe67bb03e41db5da15fec0613902af430af14a8a9f81b06'
            ]
        ]
    ]
];
