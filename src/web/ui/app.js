// ============================================
// YANDI Web UI - JavaScript
// ============================================

// === Localization (i18n) ===
const translations = {
    ru: {
        // Navigation
        nav_main: "Главная",
        nav_contacts: "Контакты",
        nav_gateways: "Шлюзы",
        nav_relay: "Relay",
        nav_settings: "Настройки",

        // Main page
        status_title: "Статус ноды",
        status_online: "🟢 Online",
        status_offline: "🔴 Offline",
        label_short_id: "Short ID",
        label_cid: "CID",
        label_role: "Роль",
        label_type: "Тип",
        label_local: "Локальная",
        label_remote: "Удалённая",
        label_external_ip: "Внешний IP",
        label_virtual_ipv6: "Виртуальный IPv6",
        label_ipv6_short: "IPv6 Short",
        label_ports: "Порты",
        label_nat_status: "NAT статус",

        btn_share_contact: "📋 Поделиться контактом",
        btn_stop_node: "⏹️ Остановить ноду",
        btn_start_node: "▶️ Запустить ноду",
        btn_refresh_nodes: "🔄 Обновить список нод",
        btn_stopping: "⏳ Остановка...",
        btn_starting: "⏳ Запуск...",

        control_title: "Управление",
        discovered_nodes: "Обнаруженные ноды",
        searching_nodes: "Поиск нод в сети...",
        no_nodes: "Нод не обнаружено.",

        btn_connect: "🔗 Connect",
        btn_disconnect: "🔴 Отключить",
        btn_proxy: "🌐 Proxy",
        btn_socks5: "🧦 SOCKS5",
        btn_relay: "🔌 Relay",

        gateway_mode: "Gateway режим",
        btn_start_gateway: "🚀 Запустить Gateway",
        btn_stop_gateway: "⏹️ Остановить Gateway",
        gateway_desc: "Gateway режим позволяет другим нодам использовать ваше интернет-соединение",

        stat_sent: "Отправлено",
        stat_received: "Получено",
        stat_speed: "Скорость",
        stat_relay_sessions: "Relay сессии",

        // Gateways page
        gateways_title: "🌐 Шлюзы (Интернет)",
        btn_add_gateway: "➕ Добавить шлюз",
        loading_gateways: "Загрузка списка шлюзов...",
        no_gateways: "Шлюзов не найдено.",
        gateway_active: "Активен",
        label_latency: "Задержка",
        label_status: "Статус",
        status_connected: "Подключен",
        status_disconnected: "Отключен",
        latency_measuring: "Измеряется...",
        btn_connect_gateway: "🟡 Подключить",
        btn_disconnect_gateway: "🔴 Отключить",
        source_mdns: "mDNS",
        source_p2p: "P2P",
        modal_add_gateway: "Добавить шлюз",
        label_gateway_name: "Название",
        label_country: "Страна",
        alert_gateway_added: "Шлюз добавлен!",
        alert_gateway_updated: "Шлюз обновлён!",
        alert_gateway_deleted: "Шлюз удалён!",
        alert_gateway_save_failed: "Не удалось сохранить шлюз",
        alert_gateway_save_error: "Ошибка при сохранении шлюза",
        alert_gateway_delete_failed: "Не удалось удалить шлюз",
        alert_gateway_delete_error: "Ошибка при удалении шлюза",
        confirm_delete_gateway: "Удалить шлюз",
        alert_disconnecting_gateway: "Отключение от",

        // Contacts page
        contacts_title: "👥 Контакты",
        btn_add_contact: "➕ Добавить контакт",
        search_contacts: "Поиск контактов...",
        modal_add_contact: "Добавить контакт",
        modal_edit_contact: "Редактировать контакт",
        label_name: "Имя",
        label_short_id_placeholder: "Short ID (например: 0021944b)",
        btn_cancel: "Отмена",
        btn_save: "Сохранить",
        btn_chat: "💬 Чат",
        btn_files: "📁 Файлы",
        btn_call: "📞 Звонок",
        btn_edit: "✏️",

        // Settings page
        settings_title: "⚙️ Настройки",
        node_mode: "Режим ноды",
        p2p_enabled: "P2P режим",
        gateway_enabled: "Gateway режим",
        gateway_settings: "Настройки Gateway",
        auto_start_gateway: "Автозапуск Gateway",
        multi_port: "Мультипорт",
        max_clients: "Максимум клиентов",
        network_settings: "Сетевые настройки",
        discovery_port: "Порт обнаружения",
        data_port: "Порт данных",
        btn_save_settings: "💾 Сохранить",
        btn_reset: "🔄 Сбросить",
        btn_reset_settings: "Сбросить настройки",

        // Alerts
        no_contacts: "Контактов нет",
        alert_contact_added: "Контакт добавлен!",
        alert_contact_failed: "Не удалось добавить контакт",
        alert_contact_error: "Ошибка при добавлении контакта",
        alert_contact_updated: "Контакт обновлён!",
        alert_contact_update_failed: "Не удалось обновить контакт",
        alert_contact_update_error: "Ошибка при обновлении контакта",
        alert_contact_deleted: "Контакт удалён!",
        alert_contact_delete_failed: "Не удалось удалить контакт",
        alert_contact_delete_error: "Ошибка при удалении контакта",
        confirm_delete: "Удалить контакт",
        alert_connecting: "Подключение к",
        alert_connect_failed: "Не удалось подключиться",
        alert_disconnecting: "Отключение от",
        alert_disconnect_error: "Ошибка отключения",
        alert_proxy_started: "Proxy запущен на порту",
        alert_proxy_failed: "Не удалось запустить proxy",
        alert_proxy_error: "Ошибка запуска proxy",
        alert_proxy_stopped: "Proxy остановлен",
        alert_proxy_stop_failed: "Не удалось остановить proxy",
        alert_proxy_stop_error: "Ошибка остановки proxy",
        proxy_active: "Proxy Active",
        alert_gateway_started: "Gateway режим запущен!",
        alert_gateway_start_failed: "Не удалось запустить Gateway",
        alert_gateway_start_error: "Ошибка запуска Gateway",
        alert_gateway_stopped: "Gateway режим остановлен",
        alert_gateway_stop_failed: "Не удалось остановить Gateway",
        alert_gateway_stop_error: "Ошибка остановки Gateway",
        alert_settings_saved: "Настройки сохранены!",
        alert_settings_failed: "Не удалось сохранить настройки",
        alert_settings_error: "Ошибка при сохранении настроек",
        alert_settings_reset: "Настройки сброшены",
        alert_stop_failed: "Не удалось остановить ноду",
        alert_start_failed: "Не удалось запустить ноду",
        alert_contact_copied: "✅ Контакт скопирован в буфер обмена!",
        alert_contact_copy_failed: "Не удалось загрузить данные ноды",
        alert_copy_error: "Ошибка при копировании контакта",

        // Chat/Call (TODO)
        chat_coming_soon: "Чат с",
        call_coming_soon: "Звонок",
        edit_coming_soon: "Редактирование",
        will_be_soon: "будет скоро!",
        contact_info: "📇 Контакта YANDI ноды:"
    },
    en: {
        // Navigation
        nav_main: "Main",
        nav_contacts: "Contacts",
        nav_gateways: "Gateways",
        nav_relay: "Relay",
        nav_settings: "Settings",

        // Main page
        status_title: "Node Status",
        status_online: "🟢 Online",
        status_offline: "🔴 Offline",
        label_short_id: "Short ID",
        label_cid: "CID",
        label_role: "Role",
        label_type: "Type",
        label_local: "Local",
        label_remote: "Remote",
        label_external_ip: "External IP",
        label_virtual_ipv6: "Virtual IPv6",
        label_ipv6_short: "IPv6 Short",
        label_ports: "Ports",
        label_nat_status: "NAT Status",

        btn_share_contact: "📋 Share Contact",
        btn_stop_node: "⏹️ Stop Node",
        btn_start_node: "▶️ Start Node",
        btn_refresh_nodes: "🔄 Refresh Nodes",
        btn_stopping: "⏳ Stopping...",
        btn_starting: "⏳ Starting...",

        control_title: "Control",
        discovered_nodes: "Discovered Nodes",
        searching_nodes: "Searching for nodes...",
        no_nodes: "No nodes found.",

        btn_connect: "🔗 Connect",
        btn_disconnect: "🔴 Disconnect",
        btn_proxy: "🌐 Proxy",
        btn_socks5: "🧦 SOCKS5",
        btn_relay: "🔌 Relay",

        gateway_mode: "Gateway Mode",
        btn_start_gateway: "🚀 Start Gateway",
        btn_stop_gateway: "⏹️ Stop Gateway",
        gateway_desc: "Gateway mode allows other nodes to use your internet connection",

        stat_sent: "Sent",
        stat_received: "Received",
        stat_speed: "Speed",
        stat_relay_sessions: "Relay Sessions",

        // Gateways page
        gateways_title: "🌐 Gateways (Internet)",
        btn_add_gateway: "➕ Add Gateway",
        loading_gateways: "Loading gateways...",
        no_gateways: "No gateways found.",
        gateway_active: "Active",
        label_latency: "Latency",
        label_status: "Status",
        status_connected: "Connected",
        status_disconnected: "Disconnected",
        latency_measuring: "Measuring...",
        btn_connect_gateway: "🟡 Connect",
        btn_disconnect_gateway: "🔴 Disconnect",
        source_mdns: "mDNS",
        source_p2p: "P2P",
        modal_add_gateway: "Add Gateway",
        label_gateway_name: "Name",
        label_country: "Country",
        alert_gateway_added: "Gateway added!",
        alert_gateway_updated: "Gateway updated!",
        alert_gateway_deleted: "Gateway deleted!",
        alert_gateway_save_failed: "Failed to save gateway",
        alert_gateway_save_error: "Error saving gateway",
        alert_gateway_delete_failed: "Failed to delete gateway",
        alert_gateway_delete_error: "Error deleting gateway",
        confirm_delete_gateway: "Delete gateway",
        alert_disconnecting_gateway: "Disconnecting from",

        // Contacts page
        contacts_title: "👥 Contacts",
        btn_add_contact: "➕ Add Contact",
        search_contacts: "Search contacts...",
        modal_add_contact: "Add Contact",
        modal_edit_contact: "Edit Contact",
        label_name: "Name",
        label_short_id_placeholder: "Short ID (e.g: 0021944b)",
        btn_cancel: "Cancel",
        btn_save: "Save",
        btn_chat: "💬 Chat",
        btn_files: "📁 Files",
        btn_call: "📞 Call",
        btn_edit: "✏️",

        // Settings page
        settings_title: "⚙️ Settings",
        node_mode: "Node Mode",
        p2p_enabled: "P2P Mode",
        gateway_enabled: "Gateway Mode",
        gateway_settings: "Gateway Settings",
        auto_start_gateway: "Auto-start Gateway",
        multi_port: "Multi-port",
        max_clients: "Max Clients",
        network_settings: "Network Settings",
        discovery_port: "Discovery Port",
        data_port: "Data Port",
        btn_save_settings: "💾 Save",
        btn_reset: "🔄 Reset",
        btn_reset_settings: "Reset Settings",

        // Alerts
        no_contacts: "No contacts",
        alert_contact_added: "Contact added!",
        alert_contact_failed: "Failed to add contact",
        alert_contact_error: "Error adding contact",
        alert_contact_updated: "Contact updated!",
        alert_contact_update_failed: "Failed to update contact",
        alert_contact_update_error: "Error updating contact",
        alert_contact_deleted: "Contact deleted!",
        alert_contact_delete_failed: "Failed to delete contact",
        alert_contact_delete_error: "Error deleting contact",
        confirm_delete: "Delete contact",
        alert_connecting: "Connecting to",
        alert_connect_failed: "Failed to connect",
        alert_disconnecting: "Disconnecting from",
        alert_disconnect_error: "Disconnect error",
        alert_proxy_started: "Proxy started on port",
        alert_proxy_failed: "Failed to start proxy",
        alert_proxy_error: "Proxy start error",
        alert_proxy_stopped: "Proxy stopped",
        alert_proxy_stop_failed: "Failed to stop proxy",
        alert_proxy_stop_error: "Proxy stop error",
        proxy_active: "Proxy Active",
        alert_gateway_started: "Gateway mode started!",
        alert_gateway_start_failed: "Failed to start Gateway",
        alert_gateway_start_error: "Gateway start error",
        alert_gateway_stopped: "Gateway mode stopped",
        alert_gateway_stop_failed: "Failed to stop Gateway",
        alert_gateway_stop_error: "Gateway stop error",
        alert_settings_saved: "Settings saved!",
        alert_settings_failed: "Failed to save settings",
        alert_settings_error: "Error saving settings",
        alert_settings_reset: "Settings reset",
        alert_stop_failed: "Failed to stop node",
        alert_start_failed: "Failed to start node",
        alert_contact_copied: "✅ Contact copied to clipboard!",
        alert_contact_copy_failed: "Failed to load node data",
        alert_copy_error: "Error copying contact",

        // Chat/Call (TODO)
        chat_coming_soon: "Chat with",
        call_coming_soon: "Call",
        edit_coming_soon: "Editing",
        will_be_soon: "will be soon!",
        contact_info: "📇 YANDI Node Contact:"
    }
};

// Get current language or default to Russian
let currentLang = localStorage.getItem('yandi_lang') || 'ru';

function t(key) {
    return translations[currentLang][key] || key;
}

function setLanguage(lang) {
    currentLang = lang;
    localStorage.setItem('yandi_lang', lang);
    location.reload();
}

// === Mobile Menu ===
const mobileMenuBtn = document.getElementById('mobileMenuBtn');
const nav = document.querySelector('.nav');

if (mobileMenuBtn) {
    mobileMenuBtn.addEventListener('click', () => {
        nav.classList.toggle('active');

        // Animate hamburger
        const spans = mobileMenuBtn.querySelectorAll('span');
        if (nav.classList.contains('active')) {
            spans[0].style.transform = 'rotate(45deg) translateY(7px)';
            spans[1].style.opacity = '0';
            spans[2].style.transform = 'rotate(-45deg) translateY(-7px)';
        } else {
            spans[0].style.transform = '';
            spans[1].style.opacity = '';
            spans[2].style.transform = '';
        }
    });
}

// === Start/Stop Button ===
const startStopBtn = document.getElementById('startStopBtn');
let isRunning = true;

if (startStopBtn) {
    startStopBtn.addEventListener('click', async () => {
        if (isRunning) {
            // Stop node
            startStopBtn.disabled = true;
            startStopBtn.textContent = t('btn_stopping');

            try {
                const response = await fetch('/api/node/stop', {
                    method: 'POST'
                });

                if (response.ok) {
                    isRunning = false;
                    startStopBtn.textContent = t('btn_start_node');
                    startStopBtn.classList.remove('btn-danger');
                    startStopBtn.classList.add('btn-primary');

                    // Update status badge
                    const statusBadge = document.querySelector('.status-badge');
                    statusBadge.textContent = t('status_offline');
                    statusBadge.classList.remove('status-online');
                    statusBadge.classList.add('status-offline');
                }
            } catch (error) {
                console.error('Failed to stop node:', error);
                alert(t('alert_stop_failed'));
            } finally {
                startStopBtn.disabled = false;
            }
        } else {
            // Start node
            startStopBtn.disabled = true;
            startStopBtn.textContent = t('btn_starting');

            try {
                const response = await fetch('/api/node/start', {
                    method: 'POST'
                });

                if (response.ok) {
                    isRunning = true;
                    startStopBtn.textContent = t('btn_stop_node');
                    startStopBtn.classList.remove('btn-primary');
                    startStopBtn.classList.add('btn-danger');

                    // Update status badge
                    const statusBadge = document.querySelector('.status-badge');
                    statusBadge.textContent = t('status_online');
                    statusBadge.classList.remove('status-offline');
                    statusBadge.classList.add('status-online');
                }
            } catch (error) {
                console.error('Failed to start node:', error);
                alert(t('alert_start_failed'));
            } finally {
                startStopBtn.disabled = false;
            }
        }
    });
}

// === Add Contact Modal ===
const addContactBtn = document.getElementById('addContactBtn');
const addContactModal = document.getElementById('addContactModal');
const closeModalBtn = document.getElementById('closeModalBtn');
const cancelBtn = document.getElementById('cancelBtn');
const addContactForm = document.getElementById('addContactForm');

// === Edit Contact Modal ===
const editContactForm = document.getElementById('editContactForm');
const closeEditModalBtn = document.getElementById('closeEditModalBtn');
const cancelEditBtn = document.getElementById('cancelEditBtn');

if (addContactBtn) {
    addContactBtn.addEventListener('click', () => {
        addContactModal.classList.add('active');
    });
}

if (closeModalBtn) {
    closeModalBtn.addEventListener('click', () => {
        addContactModal.classList.remove('active');
    });
}

if (cancelBtn) {
    cancelBtn.addEventListener('click', () => {
        addContactModal.classList.remove('active');
    });
}

// === Edit Contact Modal Handlers ===
if (closeEditModalBtn) {
    closeEditModalBtn.addEventListener('click', () => {
        closeEditContactModal();
    });
}

if (cancelEditBtn) {
    cancelEditBtn.addEventListener('click', () => {
        closeEditContactModal();
    });
}

// Close edit modal on outside click
const editContactModal = document.getElementById('editContactModal');
if (editContactModal) {
    editContactModal.addEventListener('click', (e) => {
        if (e.target === editContactModal) {
            closeEditContactModal();
        }
    });
}

// Edit contact form submission
if (editContactForm) {
    editContactForm.addEventListener('submit', async (e) => {
        e.preventDefault();

        const id = document.getElementById('editContactId').value;
        const name = document.getElementById('editContactName').value;
        const shortId = document.getElementById('editContactShortId').value;

        try {
            const response = await fetch('/api/contacts', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    id,
                    name,
                    short_id: shortId
                })
            });

            if (response.ok) {
                // Close modal
                closeEditContactModal();

                // Reset form
                editContactForm.reset();

                // Refresh contacts list
                loadContacts();

                alert(t('alert_contact_updated'));
            } else {
                alert(t('alert_contact_update_failed'));
            }
        } catch (error) {
            console.error('Failed to update contact:', error);
            alert(t('alert_contact_update_error'));
        }
    });
}

// Close modal on outside click
if (addContactModal) {
    addContactModal.addEventListener('click', (e) => {
        if (e.target === addContactModal) {
            addContactModal.classList.remove('active');
        }
    });
}

// Add contact form submission
if (addContactForm) {
    addContactForm.addEventListener('submit', async (e) => {
        e.preventDefault();

        const name = document.getElementById('contactName').value;
        const shortId = document.getElementById('contactShortId').value;

        try {
            const response = await fetch('/api/contacts', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    name,
                    short_id: shortId
                })
            });

            if (response.ok) {
                // Close modal
                addContactModal.classList.remove('active');

                // Reset form
                addContactForm.reset();

                // Refresh contacts list
                loadContacts();

                alert(t('alert_contact_added'));
            } else {
                alert(t('alert_contact_failed'));
            }
        } catch (error) {
            console.error('Failed to add contact:', error);
            alert(t('alert_contact_error'));
        }
    });
}

// === Load Contacts ===
async function loadContacts() {
    try {
        const response = await fetch('/api/contacts');
        if (response.ok) {
            const data = await response.json();
            renderContacts(data.contacts);
        }
    } catch (error) {
        console.error('Failed to load contacts:', error);
    }
}

function renderContacts(contacts) {
    const contactsList = document.getElementById('contactsList');
    if (!contactsList) return;

    if (contacts.length === 0) {
        contactsList.innerHTML = `<p class="text-muted">${t('no_contacts')}</p>`;
        return;
    }

    contactsList.innerHTML = contacts.map(contact => `
        <div class="contact-item" data-short-id="${contact.short_id}" data-contact-id="${contact.id}">
            <div class="contact-avatar">
                <span class="avatar-initial">${contact.name.charAt(0).toUpperCase()}</span>
            </div>
            <div class="contact-info">
                <h3 class="contact-name">${contact.name}</h3>
                <p class="contact-id">${contact.short_id}</p>
                <span class="contact-status ${contact.online ? 'contact-online' : 'contact-offline'}">
                    ${contact.online ? t('status_online') : t('status_offline')}
                </span>
            </div>
            <div class="contact-actions">
                <button class="btn btn-sm btn-primary" onclick="openChat('${contact.id}', '${contact.short_id}', '${contact.name}')">${t('btn_chat')}</button>
                <button class="btn btn-sm btn-info" onclick="openFileTransfer('${contact.short_id}', '${contact.name}')">${t('btn_files')}</button>
                <button class="btn btn-sm btn-secondary" onclick="startCall('${contact.short_id}')">${t('btn_call')}</button>
                <button class="btn btn-sm btn-p2p" id="p2p-btn-${contact.short_id}" onclick="startP2PTunnel('${contact.short_id}')">🔗 P2P</button>
                <button class="btn btn-sm btn-icon" onclick="openEditContactModal('${contact.id}', '${contact.name}', '${contact.short_id}')">${t('btn_edit')}</button>
                <button class="btn btn-sm btn-icon btn-danger" onclick="deleteContact('${contact.id}', '${contact.name}')">🗑️</button>
            </div>
        </div>
    `).join('');

    // Проверить статус P2P тоннелей для всех контактов
    checkP2PTunnelsStatus(contacts);
}

// === Contact Actions ===
function openChat(contactId, shortId, name) {
    console.log('Opening chat with:', { contactId, shortId, name });
    // Перейти на страницу чата с параметрами контакта
    window.location.href = `/chat#contact=${contactId}&name=${encodeURIComponent(name)}&short_id=${shortId}`;
}

// Проверить статус P2P тоннелей для всех контактов
async function checkP2PTunnelsStatus(contacts) {
    for (const contact of contacts) {
        try {
            const response = await fetch(`/api/tunnel/status/${contact.short_id}`);
            if (response.ok) {
                const data = await response.json();
                const btn = document.getElementById(`p2p-btn-${contact.short_id}`);
                const contactItem = document.querySelector(`[data-short-id="${contact.short_id}"]`);
                const statusElement = contactItem?.querySelector('.contact-status');

                if (btn) {
                    if (data.status === 'success' && data.tunnel.tunnel_status === 'Established') {
                        // Тоннель активен - красная кнопка (закрыть)
                        btn.classList.remove('btn-secondary', 'btn-p2p', 'btn-success');
                        btn.classList.add('btn-danger', 'btn-p2p-active');
                        btn.innerHTML = '❌ P2P';

                        // Обновить статус контакта на Online
                        if (statusElement && !statusElement.classList.contains('contact-online')) {
                            statusElement.classList.remove('contact-offline');
                            statusElement.classList.add('contact-online');
                            statusElement.textContent = t('status_online');
                        }
                    } else {
                        // Тоннель не активен - серая кнопка (создать)
                        btn.classList.remove('btn-danger', 'btn-p2p-active', 'btn-success');
                        btn.classList.add('btn-secondary', 'btn-p2p');
                        btn.innerHTML = '🔗 P2P';
                    }
                }
            }
        } catch (error) {
            console.error(`Failed to check P2P tunnel status for ${contact.short_id}:`, error);
        }
    }
}

function startCall(shortId) {
    console.log('Starting call with:', shortId);
    // TODO: Implement VoIP call
    alert(`${t('call_coming_soon')} ${shortId} ${t('will_be_soon')}`);
}

function startP2PTunnel(shortId) {
    console.log('Starting P2P tunnel with:', shortId);

    // Проверить текущий статус тоннеля
    const btn = document.getElementById(`p2p-btn-${shortId}`);
    const isActive = btn && btn.classList.contains('btn-p2p-active');

    if (isActive) {
        // Тоннель активен - закрыть его
        fetch(`/api/tunnel/stop/${shortId}`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            }
        })
        .then(response => response.json())
        .then(data => {
            if (data.status === 'success') {
                console.log('✅ P2P tunnel closed:', data.message);
                // Обновить статус кнопки
                checkP2PTunnelsStatus([{ short_id: shortId }]);
            } else {
                alert(`❌ Ошибка: ${data.message}`);
            }
        })
        .catch(error => {
            console.error('Error closing P2P tunnel:', error);
            alert(`❌ Ошибка закрытия тоннеля: ${error}`);
        });
    } else {
        // Тоннель не активен - создать (универсальный)
        fetch(`/api/tunnel/start/${shortId}`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({
                type: 'generic'
            })
        })
        .then(response => response.json())
        .then(data => {
            if (data.status === 'success') {
                console.log('✅ P2P tunnel started:', data.message);
                // Обновить статус кнопки
                checkP2PTunnelsStatus([{ short_id: shortId }]);
            } else {
                alert(`❌ Ошибка: ${data.message}`);
            }
        })
        .catch(error => {
            console.error('Error starting P2P tunnel:', error);
            alert(`❌ Ошибка создания тоннеля: ${error}`);
        });
    }
}

// === Edit Contact ===
function openEditContactModal(contactId, name, shortId) {
    const editContactModal = document.getElementById('editContactModal');
    if (!editContactModal) {
        alert('Edit modal not found!');
        return;
    }

    // Заполняем форму
    document.getElementById('editContactId').value = contactId;
    document.getElementById('editContactName').value = name;
    document.getElementById('editContactShortId').value = shortId;

    // Показываем модальное окно
    editContactModal.classList.add('active');
}

function closeEditContactModal() {
    const editContactModal = document.getElementById('editContactModal');
    if (editContactModal) {
        editContactModal.classList.remove('active');
    }
}

// === Delete Contact ===
async function deleteContact(contactId, name) {
    // Запрашиваем подтверждение
    const confirmed = confirm(`${t('confirm_delete')} "${name}"?`);
    if (!confirmed) return;

    try {
        const response = await fetch('/api/contacts', {
            method: 'DELETE',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ id: contactId })
        });

        if (response.ok) {
            alert(t('alert_contact_deleted'));
            // Перезагружаем список контактов
            loadContacts();
        } else {
            alert(t('alert_contact_delete_failed'));
        }
    } catch (error) {
        console.error('Failed to delete contact:', error);
        alert(t('alert_contact_delete_error'));
    }
}

// === Gateway Modal Handlers ===
const addGatewayBtn = document.getElementById('addGatewayBtn');
const addGatewayModal = document.getElementById('addGatewayModal');
const closeGatewayModalBtn = document.getElementById('closeGatewayModalBtn');
const cancelGatewayBtn = document.getElementById('cancelGatewayBtn');
const addGatewayForm = document.getElementById('addGatewayForm');

if (addGatewayBtn) {
    addGatewayBtn.addEventListener('click', () => {
        // Сбрасываем форму для добавления нового шлюза
        document.getElementById('gatewayId').value = '';
        addGatewayForm.reset();
        addGatewayModal.classList.add('active');
    });
}

if (closeGatewayModalBtn) {
    closeGatewayModalBtn.addEventListener('click', () => {
        addGatewayModal.classList.remove('active');
    });
}

if (cancelGatewayBtn) {
    cancelGatewayBtn.addEventListener('click', () => {
        addGatewayModal.classList.remove('active');
    });
}

// Close gateway modal on outside click
if (addGatewayModal) {
    addGatewayModal.addEventListener('click', (e) => {
        if (e.target === addGatewayModal) {
            addGatewayModal.classList.remove('active');
        }
    });
}

// Add/Edit gateway form submission
if (addGatewayForm) {
    addGatewayForm.addEventListener('submit', async (e) => {
        e.preventDefault();

        const id = document.getElementById('gatewayId').value;
        const name = document.getElementById('gatewayName').value;
        const shortId = document.getElementById('gatewayShortId').value;
        const country = document.getElementById('gatewayCountry').value;

        try {
            const response = await fetch('/api/gateways', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    id: id || null,
                    name,
                    short_id: shortId,
                    country
                })
            });

            if (response.ok) {
                // Close modal
                addGatewayModal.classList.remove('active');

                // Reset form
                addGatewayForm.reset();

                // Refresh gateways list
                loadGatewaysList();

                alert(id ? t('alert_gateway_updated') : t('alert_gateway_added'));
            } else {
                alert(t('alert_gateway_save_failed'));
            }
        } catch (error) {
            console.error('Failed to save gateway:', error);
            alert(t('alert_gateway_save_error'));
        }
    });
}

// === Gateway Actions ===
function openEditGatewayModal(gatewayId, name, shortId, country) {
    // Заполняем форму
    document.getElementById('gatewayId').value = gatewayId;
    document.getElementById('gatewayName').value = name;
    document.getElementById('gatewayShortId').value = shortId;
    document.getElementById('gatewayCountry').value = country;

    // Показываем модальное окно
    addGatewayModal.classList.add('active');
}

function editGateway(gatewayId, name, shortId, country) {
    openEditGatewayModal(gatewayId, name, shortId, country);
}

async function deleteGateway(gatewayId, name) {
    // Запрашиваем подтверждение
    const confirmed = confirm(`${t('confirm_delete_gateway')} "${name}"?`);
    if (!confirmed) return;

    try {
        const response = await fetch('/api/gateways', {
            method: 'DELETE',
            headers: {
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ id: gatewayId })
        });

        if (response.ok) {
            alert(t('alert_gateway_deleted'));
            // Перезагружаем список шлюзов
            loadGatewaysList();
        } else {
            alert(t('alert_gateway_delete_failed'));
        }
    } catch (error) {
        console.error('Failed to delete gateway:', error);
        alert(t('alert_gateway_delete_error'));
    }
}

function disconnectGateway(shortId) {
    // TODO: Реализовать отключение
    alert(`${t('alert_disconnecting_gateway')} ${shortId}...`);
}

// === Settings ===
const saveSettingsBtn = document.getElementById('saveSettingsBtn');
const resetBtn = document.getElementById('resetBtn');

if (saveSettingsBtn) {
    saveSettingsBtn.addEventListener('click', async () => {
        const settings = {
            node_mode: {
                p2p_enabled: document.getElementById('p2pEnabled').checked,
                gateway_enabled: document.getElementById('gatewayEnabled').checked
            },
            gateway: {
                auto_start: document.getElementById('autoStartGateway').checked,
                multi_port: document.getElementById('multiPortEnabled').checked,
                max_clients: parseInt(document.getElementById('maxClients').value)
            },
            network: {
                discovery_port: parseInt(document.getElementById('discoveryPort').value),
                data_port: parseInt(document.getElementById('dataPort').value),
                mobile_gateway: parseInt(document.getElementById('mobileGatewayPort').value),
                mobile_p2p: parseInt(document.getElementById('mobileP2PPort').value),
                http_proxy: parseInt(document.getElementById('httpProxyPort').value),
                web_ui: parseInt(document.getElementById('webUIPort').value)
            }
        };

        try {
            const response = await fetch('/api/settings', {
                method: 'PUT',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify(settings)
            });

            if (response.ok) {
                const result = await response.json();
                if (result.requires_restart) {
                    alert(t('alert_settings_saved') + '\n\n' + (result.message || ''));
                } else {
                    alert(t('alert_settings_saved'));
                }
            } else {
                alert(t('alert_settings_failed'));
            }
        } catch (error) {
            console.error('Failed to save settings:', error);
            alert(t('alert_settings_error'));
        }
    });
}

if (resetBtn) {
    resetBtn.addEventListener('click', () => {
        if (confirm(t('btn_reset_settings'))) {
            // Reset form to defaults
            document.getElementById('p2pEnabled').checked = true;
            document.getElementById('gatewayEnabled').checked = true;
            document.getElementById('autoStartGateway').checked = true;
            document.getElementById('multiPortEnabled').checked = false;
            document.getElementById('maxClients').value = 11;
            document.getElementById('discoveryPort').value = 9000;
            document.getElementById('dataPort').value = 10000;
            document.getElementById('mobileGatewayPort').value = 9111;
            document.getElementById('mobileP2PPort').value = 9112;
            document.getElementById('httpProxyPort').value = 8080;
            document.getElementById('webUIPort').value = 9999;

            alert(t('alert_settings_reset'));
        }
    });
}

// === Search Filter ===
const searchInput = document.getElementById('searchInput');
if (searchInput) {
    searchInput.addEventListener('input', (e) => {
        const searchTerm = e.target.value.toLowerCase();
        const contactItems = document.querySelectorAll('.contact-item');

        contactItems.forEach(item => {
            const name = item.querySelector('.contact-name').textContent.toLowerCase();
            const shortId = item.querySelector('.contact-id').textContent.toLowerCase();

            if (name.includes(searchTerm) || shortId.includes(searchTerm)) {
                item.style.display = '';
            } else {
                item.style.display = 'none';
            }
        });
    });
}

// === Load Gateways ===
async function loadGateways() {
    try {
        const response = await fetch('/api/nodes');
        if (response.ok) {
            const data = await response.json();
            // Превращаем ноды в формат gateways
            const gateways = data.nodes.map(node => ({
                short_id: node.short_id,
                name: node.short_id, // Используем short_id как имя
                country_flag: node.source === 'mdns' ? '🏠' : '🌐',
                connected: node.connected || true,
                latency_ms: null, // TODO: измерить задержку
                source: node.source,
                nat_status: node.nat_status || 'Unknown'
                // ❌ УБРАЛИ address для безопасности!
            }));
            renderGateways(gateways);
        }
    } catch (error) {
        console.error('Failed to load gateways:', error);
    }
}

// Load saved gateways from gateways.json
async function loadGatewaysList() {
    try {
        const [gatewaysResponse, proxyResponse] = await Promise.all([
            fetch('/api/gateways'),
            fetch('/api/proxy/status')
        ]);

        if (gatewaysResponse.ok && proxyResponse.ok) {
            const gatewaysData = await gatewaysResponse.json();
            const proxyStatus = await proxyResponse.json();
            renderGatewaysList(gatewaysData.gateways, proxyStatus);
        }
    } catch (error) {
        console.error('Failed to load gateways list:', error);
    }
}

// Load SOCKS5 status (вызывается периодически)
async function loadSocks5Status() {
    try {
        const response = await fetch('/api/socks5/status');
        if (response.ok) {
            const data = await response.json();
            // Можно обновить UI на основе статуса
            return data;
        }
    } catch (error) {
        console.error('Failed to load SOCKS5 status:', error);
    }
    return { active: false, proxies: [] };
}

function renderGatewaysList(gateways, proxyStatus = { active: false, proxies: [] }) {
    const gatewaysList = document.getElementById('gatewaysList');
    if (!gatewaysList) return;

    if (gateways.length === 0) {
        gatewaysList.innerHTML = `<p class="text-muted">${t('no_gateways')}</p>`;
        return;
    }

    // Get the short_id of the currently active proxy (if any)
    const activeProxyShortId = proxyStatus.active && proxyStatus.proxies.length > 0
        ? proxyStatus.proxies[0].short_id
        : null;

    gatewaysList.innerHTML = gateways.map(gw => {
        const hasActiveProxy = activeProxyShortId === gw.short_id;
        const anotherProxyActive = activeProxyShortId && !hasActiveProxy;

        // Proxy button: show Disconnect if active, else show Proxy
        const proxyBtn = hasActiveProxy
            ? `<button class="btn btn-sm btn-danger" onclick="stopProxy('${gw.short_id}')">${t('btn_disconnect')}</button>`
            : `<button class="btn btn-sm btn-success" onclick="startProxy('${gw.short_id}')" ${anotherProxyActive ? 'disabled' : ''}>
                ${t('btn_proxy')}
               </button>`;

        // SOCKS5 button (всегда доступен, независимо от HTTP Proxy)
        const socks5Btn = gw.connected
            ? `<button class="btn btn-sm btn-secondary" onclick="startSocks5('${gw.short_id}')">
                ${t('btn_socks5')}
               </button>`
            : `<button class="btn btn-sm btn-secondary" disabled title="Connect first">
                ${t('btn_socks5')}
               </button>`;

        // Relay button (для пиров за NAT)
        const relayBtn = gw.nat_status === 'BehindNAT' || gw.nat_status === 'Unknown'
            ? `<button class="btn btn-sm btn-warning" onclick="connectViaRelay('${gw.short_id}')" style="background: #f59e0b; color: white;">
                🔌 Relay
               </button>`
            : '';

        // Proxy badge when active (показываем тип прокси)
        const activeProxy = proxyStatus.active && proxyStatus.proxies.find(p => p.short_id === gw.short_id);
        const proxyBadge = activeProxy
            ? `<span class="gateway-badge" style="background: ${activeProxy.proxy_type === 'Socks5' ? '#bfdbfe' : '#fef08a'}; color: ${activeProxy.proxy_type === 'Socks5' ? '#1e40af' : '#854d0e'};">
                ${activeProxy.proxy_type === 'Socks5' ? '🧦' : '🌐'} ${activeProxy.proxy_type}
               </span>`
            : '';

        // NAT status badge
        const natBadge = gw.nat_status === 'Public' 
            ? '<span class="gateway-badge" style="background: #dcfce7; color: #166534;">🌐 Public</span>'
            : gw.nat_status === 'BehindNAT'
                ? '<span class="gateway-badge" style="background: #fee2e2; color: #991b1b;">🔒 Behind NAT</span>'
                : gw.nat_status === 'MultiHomed'
                    ? '<span class="gateway-badge" style="background: #fef3c7; color: #92400e;">🔄 Multi-homed</span>'
                    : '';

        return `
        <div class="gateway-item ${gw.connected ? 'gateway-connected' : ''}" data-gateway-id="${gw.id}">
            <div class="gateway-header">
                <div class="gateway-info">
                    <h3 class="gateway-name">${gw.name} ${gw.connected ? '⭐' : ''}</h3>
                    <span class="gateway-id">${gw.short_id}</span>
                    <span class="gateway-flag">${getCountryFlag(gw.country)}</span>
                    <span class="gateway-source">${gw.country}</span>
                    ${natBadge}
                    ${gw.connected ? `<span class="gateway-badge">${t('gateway_active')}</span>` : ''}
                    ${proxyBadge}
                </div>
                <div class="gateway-actions">
                    ${gw.connected
                        ? `<button class="btn btn-danger btn-disconnect" onclick="disconnectGateway('${gw.short_id}')">${t('btn_disconnect_gateway')}</button>`
                        : `<button class="btn btn-primary btn-connect" onclick="connectToNode('${gw.short_id}')">${t('btn_connect_gateway')}</button>`
                    }
                    ${proxyBtn}
                    ${socks5Btn}
                    ${relayBtn}
                    <button class="btn btn-sm btn-icon" onclick="editGateway('${gw.id}', '${gw.name}', '${gw.short_id}', '${gw.country}')">${t('btn_edit')}</button>
                    <button class="btn btn-sm btn-icon btn-danger" onclick="deleteGateway('${gw.id}', '${gw.name}')">🗑️</button>
                </div>
            </div>
            <div class="gateway-stats">
                <div class="gateway-stat">
                    <span class="stat-label">${t('label_latency')}</span>
                    <span class="stat-value">${t('latency_measuring')}</span>
                </div>
                <div class="gateway-stat">
                    <span class="stat-label">${t('label_status')}</span>
                    <span class="stat-value ${gw.connected ? 'stat-connected' : 'stat-disconnected'}">
                        ${gw.connected ? t('status_connected') : t('status_disconnected')}
                    </span>
                </div>
            </div>
        </div>
    `;
    }).join('');
}

function getCountryFlag(country) {
    const flags = {
        'RU': '🇷🇺',
        'US': '🇺🇸',
        'NL': '🇳🇱',
        'DE': '🇩🇪',
        'FR': '🇫🇷',
        'GB': '🇬🇧',
        'CA': '🇨🇦',
        'AU': '🇦🇺',
        'JP': '🇯🇵',
        'CN': '🇨🇳',
        'IN': '🇮🇳',
        'BR': '🇧🇷',
        'ZA': '🇿🇦',
        'KR': '🇰🇷',
        'IT': '🇮🇹',
        'ES': '🇪🇸',
        'PL': '🇵🇱',
        'UA': '🇺🇦',
        'KZ': '🇰🇿',
        'BY': '🇧🇾',
        'TR': '🇹🇷',
        'SE': '🇸🇪',
        'NO': '🇳🇴',
        'FI': '🇫🇮',
        'CH': '🇨🇭',
        'AT': '🇦🇹',
        'BE': '🇧🇪',
        'DK': '🇩🇰',
        'CZ': '🇨🇿',
        'GR': '🇬🇷',
        'PT': '🇵🇹',
        'IE': '🇮🇪',
        'IS': '🇮🇸',
        'LI': '🇱🇮',
        'LU': '🇱🇺',
        'MC': '🇲🇨',
        'MT': '🇲🇹',
        'SM': '🇸🇲',
        'VA': '🇻🇦',
    };
    return flags[country] || '🌐';
}

// === Load Settings ===
async function loadSettings() {
    try {
        const response = await fetch('/api/settings');
        if (response.ok) {
            const settings = await response.json();

            // Заполняем форму
            document.getElementById('p2pEnabled').checked = settings.node_mode?.p2p_enabled ?? true;
            document.getElementById('gatewayEnabled').checked = settings.node_mode?.gateway_enabled ?? true;
            document.getElementById('autoStartGateway').checked = settings.gateway?.auto_start ?? true;
            document.getElementById('multiPortEnabled').checked = settings.gateway?.multi_port ?? false;
            document.getElementById('maxClients').value = settings.gateway?.max_clients ?? 11;
            document.getElementById('discoveryPort').value = settings.network?.discovery_port ?? 9000;
            document.getElementById('dataPort').value = settings.network?.data_port ?? 10000;
            document.getElementById('mobileGatewayPort').value = settings.network?.mobile_gateway ?? 9111;
            document.getElementById('mobileP2PPort').value = settings.network?.mobile_p2p ?? 9112;
            document.getElementById('httpProxyPort').value = settings.network?.http_proxy ?? 8080;
            document.getElementById('webUIPort').value = settings.network?.web_ui ?? 9999;
        }
    } catch (error) {
        console.error('Failed to load settings:', error);
    }
}

function renderGateways(gateways) {
    const gatewaysList = document.getElementById('gatewaysList');
    if (!gatewaysList) return;

    if (gateways.length === 0) {
        gatewaysList.innerHTML = `<p class="text-muted">${t('no_gateways')}</p>`;
        return;
    }

    gatewaysList.innerHTML = gateways.map(gw => `
        <div class="gateway-item ${gw.connected ? 'gateway-connected' : ''}">
            <div class="gateway-header">
                <div class="gateway-info">
                    <h3 class="gateway-name">${gw.name} ${gw.connected ? '⭐' : ''}</h3>
                    <span class="gateway-id">${gw.short_id}</span>
                    <span class="gateway-flag">${gw.country_flag}</span>
                    <span class="gateway-source">${gw.source === 'mdns' ? t('source_mdns') : t('source_p2p')}</span>
                    ${gw.connected ? `<span class="gateway-badge">${t('gateway_active')}</span>` : ''}
                    <!-- ❌ УБРАЛИ IP адрес для безопасности! -->
                </div>
                <div class="gateway-actions">
                    ${gw.connected
                        ? `<button class="btn btn-danger btn-disconnect" data-gateway-id="${gw.short_id}">${t('btn_disconnect_gateway')}</button>`
                        : `<button class="btn btn-primary btn-connect" data-gateway-id="${gw.short_id}">${t('btn_connect_gateway')}</button>`
                    }
                    <button class="btn btn-sm btn-success" onclick="startProxy('${gw.short_id}')">
                        ${t('btn_proxy')}
                    </button>
                </div>
            </div>
            <div class="gateway-stats">
                <div class="gateway-stat">
                    <span class="stat-label">${t('label_latency')}</span>
                    <span class="stat-value">${gw.latency_ms ? gw.latency_ms + 'ms' : t('latency_measuring')}</span>
                </div>
                <div class="gateway-stat">
                    <span class="stat-label">${t('label_status')}</span>
                    <span class="stat-value ${gw.connected ? 'stat-connected' : 'stat-disconnected'}">
                        ${gw.connected ? t('status_connected') : t('status_disconnected')}
                    </span>
                </div>
            </div>
        </div>
    `).join('');

    // Re-setup event handlers after rendering
    setupGatewayActions();
}

// === Setup Gateway Actions ===
function setupGatewayActions() {
    // Connect buttons
    document.querySelectorAll('.btn-connect').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            const gatewayId = e.target.dataset.gatewayId;

            try {
                const response = await fetch(`/api/connect/${gatewayId}`, {
                    method: 'POST'
                });

                if (response.ok) {
                    alert(`${t('alert_connecting')} ${gatewayId}...`);
                    // Reload gateways to update UI
                    loadGateways();
                } else {
                    alert(t('alert_connect_failed'));
                }
            } catch (error) {
                console.error('Failed to connect:', error);
                alert(t('alert_connect_failed'));
            }
        });
    });

    // Disconnect buttons
    document.querySelectorAll('.btn-disconnect').forEach(btn => {
        btn.addEventListener('click', async (e) => {
            const gatewayId = e.target.dataset.gatewayId;

            try {
                // TODO: Implement disconnect endpoint
                alert(`${t('alert_disconnecting')} ${gatewayId}...`);
                loadGateways();
            } catch (error) {
                console.error('Failed to disconnect:', error);
                alert(t('alert_disconnect_error'));
            }
        });
    });
}

// === Initialize ===
        // Relay page initialization
        if (window.location.pathname === '/relay') {
            console.log('📡 Relay page loaded');
            // Relay page uses its own script from relay.html
            // Just ensure language translations are applied
            applyTranslations();
        }
document.addEventListener('DOMContentLoaded', () => {
    console.log('YANDI Web UI loaded');

    // Load discovered nodes on main page
    if (window.location.pathname === '/') {
        loadNodes();
        loadStatus();

        // Refresh nodes every 10 seconds
        setInterval(loadNodes, 10000);
        setInterval(loadStatus, 5000);
    }

    // Load contacts if on contacts page
    if (window.location.pathname === '/contacts') {
        loadContacts();

        // Refresh P2P tunnel status every 5 seconds
        setInterval(async () => {
            const response = await fetch('/api/contacts');
            if (response.ok) {
                const data = await response.json();
                checkP2PTunnelsStatus(data.contacts);
            }
        }, 5000);
    }

    // Load gateways if on gateways page
    if (window.location.pathname === '/gateways') {
        loadGatewaysList();
        setupGatewayActions();

        // Refresh gateways every 10 seconds
        setInterval(loadGatewaysList, 10000);
    }

    // Load settings if on settings page
    if (window.location.pathname === '/settings') {
        loadSettings();
    }

    // Relay page is handled by relay.html's own script

    // Simulate live updates
    setInterval(() => {
        // Update uptime every second
        // TODO: Get real data from API
    }, 1000);
});

// === Discovered Nodes ===

// Load discovered nodes
async function loadNodes() {
    try {
        // Load nodes and proxy status in parallel
        const [nodesResponse, proxyResponse] = await Promise.all([
            fetch('/api/nodes'),
            fetch('/api/proxy/status')
        ]);

        if (nodesResponse.ok) {
            const data = await nodesResponse.json();

            // Get proxy status
            let proxyStatus = { active: false, proxies: [] };
            if (proxyResponse.ok) {
                proxyStatus = await proxyResponse.json();
            }

            renderNodes(data.nodes, proxyStatus);

            // Update node count badge
            const nodeCount = document.getElementById('nodeCount');
            if (nodeCount) {
                nodeCount.textContent = data.nodes.length;
            }
        }
    } catch (error) {
        console.error('Failed to load nodes:', error);
    }
}

// Render nodes list
function renderNodes(nodes, proxyStatus) {
    const nodesList = document.getElementById('nodesList');
    if (!nodesList) return;

    if (nodes.length === 0) {
        nodesList.innerHTML = `<p class="text-muted">${t('no_nodes')}</p>`;
        return;
    }

    // Check which node has active proxy
    const activeProxyShortId = proxyStatus.active && proxyStatus.proxies.length > 0
        ? proxyStatus.proxies[0].short_id
        : null;

    nodesList.innerHTML = nodes.map(node => {
        const sourceBadge = node.source === 'mdns'
            ? '<span class="node-badge" style="background: #dcfce7; color: #166534;">mDNS</span>'
            : '<span class="node-badge" style="background: #dbeafe; color: #1e40af;">P2P</span>';

        // Check if this node has active proxy
        const hasActiveProxy = activeProxyShortId === node.short_id;
        const proxyBtn = hasActiveProxy
            ? `<button class="btn btn-sm btn-danger" onclick="stopProxy('${node.short_id}')">🔴 Disconnect</button>`
            : `<button class="btn btn-sm btn-success" onclick="startProxy('${node.short_id}')" ${activeProxyShortId ? 'disabled' : ''}>
                ${t('btn_proxy')}
               </button>`;

        // Disable proxy button if another proxy is active
        const proxyDisabled = activeProxyShortId && !hasActiveProxy ? 'disabled style="opacity: 0.5;"' : '';

        // ✅ Проверяем connected для правильной кнопки
        const isConnected = node.connected === true;
        const connectBtn = isConnected
            ? `<button class="btn btn-sm btn-danger" onclick="disconnectFromNode('${node.short_id}')">${t('btn_disconnect')}</button>`
            : `<button class="btn btn-sm btn-primary" onclick="connectToNode('${node.short_id}')">${t('btn_connect')}</button>`;

        // Relay button (для пиров за NAT)
        const relayBtn = node.nat_status === 'BehindNAT' || node.nat_status === 'Unknown'
            ? `<button class="btn btn-sm btn-warning" onclick="connectViaRelay('${node.short_id}')" style="background: #f59e0b; color: white;">
                🔌 Relay
               </button>`
            : '';

        // NAT status badge
        const natBadge = node.nat_status === 'Public' 
            ? '<span class="node-badge" style="background: #dcfce7; color: #166534;">🌐 Public</span>'
            : node.nat_status === 'BehindNAT'
                ? '<span class="node-badge" style="background: #fee2e2; color: #991b1b;">🔒 Behind NAT</span>'
                : node.nat_status === 'MultiHomed'
                    ? '<span class="node-badge" style="background: #fef3c7; color: #92400e;">🔄 Multi-homed</span>'
                    : '';

        // Proxy badge if active
        const proxyBadge = hasActiveProxy
            ? '<span class="gateway-badge" style="background: #fef08a; color: #854d0e;">🌐 Proxy Active</span>'
            : '';

        return `
        <div class="node-item ${isConnected ? 'gateway-connected' : ''} ${hasActiveProxy ? 'proxy-active' : ''}" data-short-id="${node.short_id}">
            <div class="node-avatar">
                <span class="avatar-initial">${node.short_id.charAt(0).toUpperCase()}</span>
            </div>
            <div class="node-info">
                <h3 class="node-name">${node.hostname.replace('.local', '').replace('.unknown', '')}</h3>
                <p class="node-id">${node.short_id}</p>
                <p class="node-role">${node.role}</p>
                ${sourceBadge}
                ${natBadge}
                ${isConnected ? '<span class="gateway-badge">Активен</span>' : ''}
                ${proxyBadge}
            </div>
            <div class="node-actions">
                ${connectBtn}
                ${proxyBtn}
                ${relayBtn}
            </div>
        </div>
    `;
    }).join('');
}

// Refresh nodes button
const refreshNodesBtn = document.getElementById('refreshNodesBtn');
if (refreshNodesBtn) {
    refreshNodesBtn.addEventListener('click', () => {
        loadNodes();
    });
}

// Connect to node
async function connectToNode(shortId) {
    try {
        const response = await fetch(`/api/connect/${shortId}`, {
            method: 'POST'
        });

        if (response.ok) {
            alert(`${t('alert_connecting')} ${shortId}...`);
            // Reload nodes to update UI
            setTimeout(() => loadNodes(), 1000);
        } else {
            alert(t('alert_connect_failed'));
        }
    } catch (error) {
        console.error('Failed to connect:', error);
        alert(t('alert_connect_failed'));
    }
}

// Disconnect from node
async function disconnectFromNode(shortId) {
    try {
        // TODO: Implement real disconnect endpoint
        alert(`${t('alert_disconnecting')} ${shortId}...`);
        // Reload nodes to update UI
        setTimeout(() => loadNodes(), 1000);
    } catch (error) {
        console.error('Failed to disconnect:', error);
        alert(t('alert_disconnect_error'));
    }
}

// Start proxy through node
async function startProxy(shortId) {
    try {
        const response = await fetch(`/api/proxy/start/${shortId}`, {
            method: 'POST'
        });

        if (response.ok) {
            const data = await response.json();
            alert(`${t('alert_proxy_started')} ${data.local_port}\n🌐 Gateway: ${data.gateway}`);
            // Reload nodes to update UI
            setTimeout(() => loadNodes(), 500);
        } else {
            const error = await response.json();
            alert(`Error: ${error.message}`);
        }
    } catch (error) {
        console.error('Failed to start proxy:', error);
        alert(t('alert_proxy_error'));
    }
}

// Stop proxy
async function stopProxy(shortId) {
    try {
        const response = await fetch(`/api/proxy/stop/${shortId}`, {
            method: 'POST'
        });

        if (response.ok) {
            alert(`${t('alert_proxy_stopped')} ${shortId}`);
            // Reload nodes to update UI
            setTimeout(() => loadNodes(), 500);
        } else {
            alert(t('alert_proxy_stop_failed'));
        }
    } catch (error) {
        console.error('Failed to stop proxy:', error);
        alert(t('alert_proxy_stop_error'));
    }
}

// Start SOCKS5 proxy through node
async function startSocks5(shortId) {
    try {
        const response = await fetch(`/api/socks5/start/${shortId}`, {
            method: 'POST'
        });

        if (response.ok) {
            const data = await response.json();
            alert(`🧦 SOCKS5 запущен на порту 1080!\n🌐 Gateway: ${data.gateway}\n🔐 Auth: ${data.auth_required ? 'YES' : 'NO'}\n👤 Username: ${data.username || 'N/A'}`);
            // Reload gateways to update UI
            setTimeout(() => loadGateways(), 500);
            setTimeout(() => loadNodes(), 500);
        } else {
            alert('Failed to start SOCKS5 proxy');
        }
    } catch (error) {
        console.error('Failed to start SOCKS5:', error);
        alert('SOCKS5 start error');
    }
}

// Stop SOCKS5 proxy
async function stopSocks5(shortId) {
    try {
        const response = await fetch(`/api/socks5/stop/${shortId}`, {
            method: 'POST'
        });

        if (response.ok) {
            alert(`🧦 SOCKS5 stopped for ${shortId}`);
            // Reload nodes to update UI
            setTimeout(() => loadNodes(), 500);
            setTimeout(() => loadGateways(), 500);
        } else {
            alert('Failed to stop SOCKS5 proxy');
        }
    } catch (error) {
        console.error('Failed to stop SOCKS5:', error);
        alert('SOCKS5 stop error');
    }
}

// Connect via relay (для пиров за NAT)
async function connectViaRelay(shortId) {
    try {
        const response = await fetch(`/api/relay/connect/${shortId}`, {
            method: 'POST'
        });

        const data = await response.json();

        if (response.ok) {
            alert(`✅ ${data.message}\nSession ID: ${data.session_id}`);
            // Reload nodes to update UI
            setTimeout(() => loadNodes(), 1000);
        } else {
            alert(`❌ ${data.message}`);
        }
    } catch (error) {
        console.error('Failed to connect via relay:', error);
        alert('❌ Error connecting via relay');
    }
}

// === Gateway Control ===

const startGatewayBtn = document.getElementById('startGatewayBtn');
const stopGatewayBtn = document.getElementById('stopGatewayBtn');

if (startGatewayBtn) {
    startGatewayBtn.addEventListener('click', async () => {
        try {
            const response = await fetch('/api/gateway/start', {
                method: 'POST'
            });

            if (response.ok) {
                startGatewayBtn.style.display = 'none';
                stopGatewayBtn.style.display = 'inline-block';
                alert(t('alert_gateway_started'));
            } else {
                alert(t('alert_gateway_start_failed'));
            }
        } catch (error) {
            console.error('Failed to start gateway:', error);
            alert(t('alert_gateway_start_error'));
        }
    });
}

if (stopGatewayBtn) {
    stopGatewayBtn.addEventListener('click', async () => {
        try {
            const response = await fetch('/api/gateway/stop', {
                method: 'POST'
            });

            if (response.ok) {
                stopGatewayBtn.style.display = 'none';
                startGatewayBtn.style.display = 'inline-block';
                alert(t('alert_gateway_stopped'));
            } else {
                alert(t('alert_gateway_stop_failed'));
            }
        } catch (error) {
            console.error('Failed to stop gateway:', error);
            alert(t('alert_gateway_stop_error'));
        }
    });
}

// === Status Loading ===
async function loadStatus() {
    try {
        const response = await fetch('/api/status');
        if (response.ok) {
            const status = await response.json();

            // Update node info fields
            const shortIdEl = document.getElementById('statusShortId');
            if (shortIdEl) shortIdEl.textContent = status.short_id || '-';

            const cidEl = document.getElementById('statusCid');
            if (cidEl) cidEl.textContent = status.cid || '-';

            const roleEl = document.getElementById('statusRole');
            if (roleEl) roleEl.textContent = status.role || '-';

            const isLocalEl = document.getElementById('statusIsLocal');
            if (isLocalEl) isLocalEl.textContent = status.is_local ? 'Локальная' : 'Удалённая';

            const externalIpEl = document.getElementById('statusExternalIp');
            if (externalIpEl) externalIpEl.textContent = status.external_ip || '-';

            const virtualIpv6El = document.getElementById('statusVirtualIpv6');
            if (virtualIpv6El) virtualIpv6El.textContent = status.virtual_ipv6 || '-';

            const ipv6ShortEl = document.getElementById('statusIpv6Short');
            if (ipv6ShortEl) ipv6ShortEl.textContent = status.ipv6_short || '-';

            const portsEl = document.getElementById('statusPorts');
            if (portsEl) {
                portsEl.textContent = `D:${status.discovery_port} D:${status.data_port} W:${status.web_port}`;
            }

            const natStatusEl = document.getElementById('statusNat');
            if (natStatusEl) {
                natStatusEl.textContent = status.nat_status || 'Unknown';
            }

            // Update relay sessions count in stats
            const relaySessionsEl = document.getElementById('relaySessions');
            if (relaySessionsEl && status.relay_sessions) {
                relaySessionsEl.textContent = status.relay_sessions;
            }

            // === Update status badge ===
            const statusBadge = document.querySelector('.status-badge');
            if (statusBadge) {
                if (status.status === 'online') {
                    statusBadge.textContent = t('status_online');
                    statusBadge.classList.remove('status-offline');
                    statusBadge.classList.add('status-online');
                } else {
                    statusBadge.textContent = t('status_offline');
                    statusBadge.classList.remove('status-online');
                    statusBadge.classList.add('status-offline');
                }
            }

            // === Sync start/stop button with actual status ===
            const startStopBtn = document.getElementById('startStopBtn');
            if (startStopBtn) {
                const wasRunning = isRunning;
                isRunning = (status.status === 'online');

                // Only update UI if status changed
                if (wasRunning !== isRunning) {
                    if (isRunning) {
                        startStopBtn.textContent = t('btn_stop_node');
                        startStopBtn.classList.remove('btn-primary');
                        startStopBtn.classList.add('btn-danger');
                    } else {
                        startStopBtn.textContent = t('btn_start_node');
                        startStopBtn.classList.remove('btn-danger');
                        startStopBtn.classList.add('btn-primary');
                    }
                }
            }
        }
    } catch (error) {
        console.error('Failed to load status:', error);
    }
}

// === Share Contact ===
const shareContactBtn = document.getElementById('shareContactBtn');
if (shareContactBtn) {
    shareContactBtn.addEventListener('click', async () => {
        try {
            const response = await fetch('/api/status');
            if (response.ok) {
                const status = await response.json();

                // Format contact info
                const contactInfo = `${t('contact_info')}\n` +
                    `Short ID: ${status.short_id}\n` +
                    `CID: ${status.cid}\n` +
                    `${t('label_role')}: ${status.role}\n` +
                    `${t('label_nat_status')}: ${status.nat_status || 'Unknown'}`;

                // Copy to clipboard
                await navigator.clipboard.writeText(contactInfo);

                alert(`${t('alert_contact_copied')}\n\n${contactInfo}`);
            } else {
                alert(t('alert_contact_copy_failed'));
            }
        } catch (error) {
            console.error('Failed to share contact:', error);
            alert(t('alert_copy_error'));
        }
    });
}

// === File Transfer ===
let currentFileTransferPeer = null;

function openFileTransfer(shortId, name) {
    console.log('Opening file transfer with:', { shortId, name });

    // Сохраним текущего пира
    currentFileTransferPeer = shortId;

    // Показать модальное окно выбора файла
    const modal = document.createElement('div');
    modal.id = 'fileTransferModal';
    modal.className = 'modal active'; // Добавить active для отображения
    modal.innerHTML = `
        <div class="modal-content" style="max-width: 600px;">
            <div class="modal-header">
                <h2>📁 Отправить файл - ${name}</h2>
                <button class="close-btn" onclick="closeFileTransferModal()">✖</button>
            </div>
            <div class="modal-body">
                <div class="file-upload-area">
                    <input
                        type="file"
                        id="fileInput"
                        style="display: none;"
                        onchange="handleFileSelected(event)"
                    >
                    <div id="fileDropZone" class="file-drop-zone" onclick="document.getElementById('fileInput').click()">
                        <div class="drop-zone-icon">📁</div>
                        <div class="drop-zone-text">Нажмите для выбора файла</div>
                        <div class="drop-zone-hint">или перетащите файл сюда</div>
                    </div>
                    <div id="fileInfo" class="file-info" style="display: none;">
                        <div class="file-details">
                            <span class="file-icon">📄</span>
                            <div class="file-meta">
                                <div class="file-name" id="fileName"></div>
                                <div class="file-size" id="fileSize"></div>
                            </div>
                        </div>
                        <div class="file-actions">
                            <button class="btn btn-success" onclick="sendFile()">➤ Отправить</button>
                            <button class="btn btn-secondary" onclick="clearFile()">🗑️ Очистить</button>
                        </div>
                    </div>
                </div>
                <div id="transferProgress" class="transfer-progress" style="display: none;">
                    <div class="progress-label">Отправка файла...</div>
                    <div class="progress-bar">
                        <div class="progress-fill" id="progressFill" style="width: 0%"></div>
                    </div>
                    <div class="progress-status" id="progressStatus">0%</div>
                </div>
            </div>
        </div>
    `;

    document.body.appendChild(modal);

    // Drag & Drop handlers
    const dropZone = modal.querySelector('#fileDropZone');
    dropZone.addEventListener('dragover', (e) => {
        e.preventDefault();
        dropZone.classList.add('drag-over');
    });

    dropZone.addEventListener('dragleave', () => {
        dropZone.classList.remove('drag-over');
    });

    dropZone.addEventListener('drop', (e) => {
        e.preventDefault();
        dropZone.classList.remove('drag-over');
        const files = e.dataTransfer.files;
        if (files.length > 0) {
            document.getElementById('fileInput').files = files;
            handleFileSelected({ target: { files } });
        }
    });

    // Close on backdrop click
    modal.addEventListener('click', (e) => {
        if (e.target === modal) {
            closeFileTransferModal();
        }
    });
}

function handleFileSelected(event) {
    const file = event.target.files[0];
    if (!file) return;

    console.log('File selected:', file.name, file.size);

    // Показать информацию о файле
    document.getElementById('fileDropZone').style.display = 'none';
    document.getElementById('fileInfo').style.display = 'block';
    document.getElementById('fileName').textContent = file.name;
    document.getElementById('fileSize').textContent = formatFileSize(file.size);
}

function clearFile() {
    document.getElementById('fileInput').value = '';
    document.getElementById('fileDropZone').style.display = 'block';
    document.getElementById('fileInfo').style.display = 'none';
}

function formatFileSize(bytes) {
    if (bytes === 0) return '0 Bytes';
    const k = 1024;
    const sizes = ['Bytes', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return Math.round(bytes / Math.pow(k, i) * 100) / 100 + ' ' + sizes[i];
}

async function sendFile() {
    const fileInput = document.getElementById('fileInput');
    const file = fileInput.files[0];

    if (!file) {
        alert('Выберите файл');
        return;
    }

    if (!currentFileTransferPeer) {
        alert('Ошибка: не выбран получатель');
        return;
    }

    // Проверка размера (10 GB для кнопки Файлы)
    const maxSize = 10 * 1024 * 1024 * 1024; // 10 GB
    if (file.size > maxSize) {
        alert('❌ Файл слишком большой! Максимум 10 GB');
        return;
    }

    console.log('📤 Sending file:', file.name, formatFileSize(file.size), 'to', currentFileTransferPeer);

    // Показать прогресс
    document.getElementById('fileInfo').style.display = 'none';
    document.getElementById('transferProgress').style.display = 'block';

    try {
        // Отправляем файл целиком (сервер сам чанкует)
        const formData = new FormData();
        formData.append("file", file);
        formData.append("filename", file.name);
        formData.append("mime_type", file.type || "application/octet-stream");

        document.getElementById("progressFill").style.width = "50%";
        document.getElementById("progressStatus").textContent = "50% - Отправка файла...";

        const response = await fetch(`/api/files/send-file/${currentFileTransferPeer}`, {
            method: "POST",
            body: formData
        });

        const data = await response.json();

        if (data.status !== "success") {
            alert("❌ Ошибка отправки файла: " + data.message);
            document.getElementById("transferProgress").style.display = "none";
            document.getElementById("fileInfo").style.display = "block";
            return;
        }

        document.getElementById("progressFill").style.width = "100%";
        document.getElementById("progressStatus").textContent = "100% - Файл отправлен!";

        setTimeout(() => {
            alert(`✅ Файл "${file.name}" (${formatFileSize(file.size)}) отправлен!`);
            closeFileTransferModal();
        }, 500);

        // Прогресс 100%
        document.getElementById('progressFill').style.width = '100%';
        document.getElementById('progressStatus').textContent = '100% - Запущена P2P передача!';

        setTimeout(() => {
            alert(`✅ Файл "${file.name}" (${formatFileSize(file.size)}) отправлен!`);
            closeFileTransferModal();
        }, 500);
    } catch (error) {
        console.error('Failed to send file:', error);
        alert('❌ Ошибка отправки файла: ' + error.message);
        document.getElementById('transferProgress').style.display = 'none';
        document.getElementById('fileInfo').style.display = 'block';
    }
}

function closeFileTransferModal() {
    const modal = document.getElementById('fileTransferModal');
    if (modal) {
        modal.remove();
    }
    currentFileTransferPeer = null;
}

// === Sound System (глобальный) ===
let soundEnabled = localStorage.getItem('soundEnabled') !== 'false';
let soundElements = {};

function loadSounds() {
    if (!soundElements.icq) {
        soundElements.icq = new Audio('/media/icq.mp3');
        soundElements.icq.volume = 0.4;
        soundElements.online = new Audio('/media/icq-online.mp3');
        soundElements.online.volume = 0.3;
    }
}

function playMessageSound() {
    if (!soundEnabled) return;
    try {
        loadSounds();
        soundElements.icq.currentTime = 0;
        soundElements.icq.play().catch(e => console.log('Audio error:', e));
    } catch(e) {}
}

function playOnlineSound() {
    if (!soundEnabled) return;
    try {
        loadSounds();
        soundElements.online.currentTime = 0;
        soundElements.online.play().catch(e => console.log('Audio error:', e));
    } catch(e) {}
}

function toggleSound() {
    soundEnabled = !soundEnabled;
    localStorage.setItem('soundEnabled', soundEnabled);
    const btns = document.querySelectorAll('#soundToggle');
    btns.forEach(btn => {
        btn.textContent = soundEnabled ? '🔔' : '🔕';
    });
    console.log('🔊 Sound toggled:', soundEnabled ? 'ON' : 'OFF');
}

// Инициализация кнопок
function initSoundButtons() {
    const btns = document.querySelectorAll('#soundToggle');
    console.log('🔊 Found sound buttons:', btns.length);
    btns.forEach(btn => {
        btn.textContent = soundEnabled ? '🔔' : '🔕';
        // Убираем старые обработчики
        btn.removeEventListener('click', toggleSound);
        btn.addEventListener('click', toggleSound);
    });
}

// Запускаем инициализацию
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initSoundButtons);
} else {
    initSoundButtons();
}

// Экспортируем для использования в других скриптах
window.playMessageSound = playMessageSound;
window.playOnlineSound = playOnlineSound;
window.toggleSound = toggleSound;
window.isSoundEnabled = function() { return soundEnabled; };

// === Apply translations (глобальная функция) ===
function applyTranslations() {
    document.querySelectorAll('[data-i18n]').forEach(el => {
        const key = el.getAttribute('data-i18n');
        if (window.t && translations[currentLang] && translations[currentLang][key]) {
            el.textContent = translations[currentLang][key];
        }
    });
    
    document.querySelectorAll('[data-i18n-placeholder]').forEach(el => {
        const key = el.getAttribute('data-i18n-placeholder');
        if (window.t && translations[currentLang] && translations[currentLang][key]) {
            el.setAttribute('placeholder', translations[currentLang][key]);
        }
    });
}

// Экспортируем глобально
window.applyTranslations = applyTranslations;
