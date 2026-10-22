//! Tiny ru/en string table. English text is the key.

use std::sync::OnceLock;

static RU: OnceLock<bool> = OnceLock::new();

/// `setting` is "auto", "ru" or "en".
pub fn init(setting: &str) {
    let ru = match setting {
        "ru" => true,
        "en" => false,
        _ => ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|v| std::env::var(v).ok())
            .find(|v| !v.is_empty())
            .is_some_and(|v| v.starts_with("ru")),
    };
    let _ = RU.set(ru);
}

pub fn tr(en: &'static str) -> &'static str {
    if !RU.get().copied().unwrap_or(false) {
        return en;
    }
    match en {
        "No file is open" => "Нет открытого файла",
        "Open a note from the sidebar or create a new one" => "Откройте заметку слева или создайте новую",
        "New note" => "Новая заметка",
        "Untitled" => "Без названия",
        "New tab" => "Новая вкладка",
        "Toggle sidebar" => "Боковая панель",
        "Settings" => "Настройки",
        "Theme" => "Тема",
        "System" => "Системная",
        "Light" => "Светлая",
        "Dark" => "Тёмная",
        "Gruvbox Dark" => "Gruvbox тёмная",
        "Gruvbox Light" => "Gruvbox светлая",
        "Language" => "Язык",
        "Automatic" => "Автоматически",
        "Restart the app to change the language" => "Перезапустите приложение, чтобы сменить язык",
        "Open folder as vault…" => "Открыть папку как хранилище…",
        "Open vault" => "Открыть хранилище",
        "No vault" => "Нет хранилища",
        "Open a folder with notes — an Obsidian vault works as is" => {
            "Откройте папку с заметками — хранилище Obsidian подойдёт как есть"
        }
        "Canvas view is coming in a later version" => "Просмотр канвасов появится в следующей версии",
        "Cannot open file" => "Не удалось открыть файл",
        "Cannot save file" => "Не удалось сохранить файл",
        "Words" => "Слов",
        "Characters" => "Символов",
        "Close tab" => "Закрыть вкладку",
        "Files" => "Файлы",
        "Search" => "Поиск",
        "Backlinks" => "Обратные ссылки",
        "Search in vault…" => "Поиск по хранилищу…",
        "Nothing found" => "Ничего не найдено",
        "No backlinks" => "Нет обратных ссылок",
        "Quick switcher" => "Быстрый переход",
        "Type a note name…" => "Введите имя заметки…",
        "Create note" => "Создать заметку",
        "Editor" => "Редактор",
        "Editor font…" => "Шрифт редактора…",
        "Use vault font" => "Шрифт из хранилища",
        "Headings" => "Заголовки",
        "Large" => "Крупные",
        "Compact" => "Компактные",
        _ => en,
    }
}
