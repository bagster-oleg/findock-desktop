//! FinDock для компьютера — тонкая оболочка над кабинетом cp.findock.ru.
//!
//! Что делает сама оболочка (всё остальное — сайт внутри окна):
//! - открывает кабинет в своём окне, сессия живёт между запусками (хранилище webview);
//! - скачивание документов кладёт в «Загрузки» и показывает файл в Finder/Проводнике;
//! - внешние ссылки и `target=_blank` уводит в системный браузер, в окне остаются только наши
//!   хосты и страницы входа провайдеров подключений (Google, Яндекс);
//! - второй запуск не плодит окна, а поднимает первое;
//! - при старте тихо проверяет и ставит обновление самой оболочки.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;
use url::Url;

/// Адрес кабинета. Переопределяется на этапе сборки: `FINDOCK_URL=https://dev.findock.ru/`.
const APP_URL: &str = match option_env!("FINDOCK_URL") {
    Some(url) => url,
    None => "https://cp.findock.ru/",
};

/// Хосты, которые остаются внутри окна: наши + страницы входа провайдеров подключений.
const IN_APP_HOST_SUFFIXES: &[&str] = &["findock.ru", "google.com", "yandex.ru", "yandex.com"];

/// Пути скачиваемых файлов по URL: на macOS событие `Finished` пути не несёт, запоминаем сами.
#[derive(Default)]
struct Downloads(Mutex<HashMap<String, PathBuf>>);

pub fn run() {
    tauri::Builder::default()
        // Плагин единственного экземпляра должен идти первым.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Downloads::default())
        .setup(|app| {
            open_main_window(app.handle())?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { update_quietly(handle).await });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("не удалось запустить FinDock");
}

fn open_main_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let url: Url = APP_URL.parse().expect("APP_URL должен быть корректным адресом");
    let nav_handle = app.clone();
    let new_window_handle = app.clone();

    WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
        .title("FinDock")
        .inner_size(1280.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .on_navigation(move |url| {
            if stays_in_app(url) {
                return true;
            }
            let _ = nav_handle.opener().open_url(url.as_str(), None::<&str>);
            false
        })
        .on_new_window(move |url, _features| {
            // Ссылки с target=_blank — в системный браузер, даже если это наш хост:
            // новое окно оболочки без адресной строки и кнопки «назад» только запутает.
            let _ = new_window_handle.opener().open_url(url.as_str(), None::<&str>);
            NewWindowResponse::Deny
        })
        .on_download(|webview, event| {
            match event {
                DownloadEvent::Requested { url, destination } => {
                    let target = download_target(&webview, destination);
                    webview
                        .state::<Downloads>()
                        .0
                        .lock()
                        .unwrap()
                        .insert(url.to_string(), target.clone());
                    *destination = target;
                }
                DownloadEvent::Finished { url, path, success } => {
                    let remembered = webview
                        .state::<Downloads>()
                        .0
                        .lock()
                        .unwrap()
                        .remove(&url.to_string());
                    if success {
                        if let Some(file) = path.or(remembered) {
                            let _ = webview.opener().reveal_item_in_dir(file);
                        }
                    }
                }
                _ => {}
            }
            true
        })
        .build()?;

    Ok(())
}

/// Наш ли это хост (или его поддомен).
fn stays_in_app(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true; // about:blank и прочее служебное
    };
    IN_APP_HOST_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// Куда класть скачанный файл: «Загрузки» + имя, предложенное сайтом; занято — добавляем (1), (2)…
fn download_target<R: Runtime>(webview: &tauri::Webview<R>, suggested: &Path) -> PathBuf {
    let dir = webview
        .path()
        .download_dir()
        .or_else(|_| webview.path().home_dir())
        .unwrap_or_else(|_| std::env::temp_dir());
    let name = suggested
        .file_name()
        .map(|n| n.to_os_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "document".into());
    let name = Path::new(&name);
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into());
    let ext = name
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let mut candidate = dir.join(name);
    let mut n = 1;
    while candidate.exists() {
        candidate = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    candidate
}

/// Тихое обновление оболочки. Любая ошибка (нет сети, GitHub недоступен) — просто нет обновления,
/// кабинет работает как обычно; сайт внутри окна от версии оболочки не зависит.
async fn update_quietly<R: Runtime>(app: AppHandle<R>) {
    let Ok(updater) = app.updater() else { return };
    let Ok(Some(update)) = updater.check().await else { return };
    if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
        // Windows: установщик сам закрывает и перезапускает приложение.
        // macOS: новый бандл уже на месте — перезапускаемся, чтобы не работать из старого.
        #[cfg(target_os = "macos")]
        app.restart();
    }
}
