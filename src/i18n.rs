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
        "New folder" => "Новая папка",
        "Open in new tab" => "Открыть в новой вкладке",
        "Rename…" => "Переименовать…",
        "Rename" => "Переименовать",
        "Delete" => "Удалить",
        "Cancel" => "Отмена",
        "Invalid name" => "Недопустимое имя",
        "The folder and everything in it will be deleted." => "Папка и всё её содержимое будут удалены.",
        "Cannot delete" => "Не удалось удалить",
        "Cannot move" => "Не удалось переместить",
        "Links updated in notes" => "Ссылки обновлены в заметках",
        "Bookmarks" => "Закладки",
        "Tags" => "Теги",
        "Outline" => "Оглавление",
        "Bookmark" => "В закладки",
        "Remove from bookmarks" => "Убрать из закладок",
        "Added to bookmarks" => "Добавлено в закладки",
        "Removed from bookmarks" => "Убрано из закладок",
        "No bookmarks" => "Нет закладок",
        "No tags" => "Нет тегов",
        "No headings" => "Нет заголовков",
        "Vim mode" => "Vim-режим",
        "Vim mode on" => "Vim-режим включён",
        "Vim mode off" => "Vim-режим выключен",
        "Command palette" => "Палитра команд",
        "Type a command…" => "Введите команду…",
        "Delete current note" => "Удалить текущую заметку",
        "Bookmark current note" => "Закладка на текущую заметку",
        "Show files" => "Показать файлы",
        "Show bookmarks" => "Показать закладки",
        "Show tags" => "Показать теги",
        "Show outline" => "Показать оглавление",
        "Show backlinks" => "Показать обратные ссылки",
        "Save" => "Сохранить",
        _ => en,
    }
}
