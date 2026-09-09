//! FinDock для компьютера — тонкая оболочка над кабинетом cp.findock.ru.
//!
//! Что делает сама оболочка (всё остальное — сайт внутри окна):
//! - открывает кабинет в своём окне, сессия живёт между запусками (хранилище webview);
//! - скачивание документов кладёт в «Загрузки» и показывает файл в Finder/Проводнике;
//! - ссылки `target=_blank` на наш домен открывает в отдельном окне приложения (там та же
//!   сессия), чужие адреса — в системном браузере; в главном окне остаются наши домены и
//!   страницы входа провайдеров подключений (Google, Яндекс);
//! - второй запуск не плодит окна, а поднимает первое;
//! - при старте тихо скачивает обновление оболочки и спрашивает, перезапустить ли сейчас.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::webview::{DownloadEvent, NewWindowResponse};
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;
use url::Url;

/// Адрес кабинета. Переопределяется на этапе сборки: `FINDOCK_URL=https://dev.findock.ru/`.
const APP_URL: &str = match option_env!("FINDOCK_URL") {
    Some(url) => url,
    None => "https://cp.findock.ru/",
};

/// Наши домены (и поддомены): остаются в окне, `_blank` на них — отдельное окно приложения.
const OWN_HOST_SUFFIXES: &[&str] = &["findock.ru"];

/// Страницы входа провайдеров подключений — остаются в главном окне.
const AUTH_HOSTS: &[&str] = &[
    "accounts.google.com",
    "accounts.youtube.com",
    "oauth.yandex.ru",
    "passport.yandex.ru",
    "passport.yandex.com",
    "sso.passport.yandex.ru",
];

/// Ресурсы страниц входа (капчи, статика). На macOS webview спрашивает разрешение и для
/// фреймов, не только для главной навигации, — отказ сломал бы капчу и открыл бы Safari.
const AUTH_RESOURCE_SUFFIXES: &[&str] = &[
    "google.com",
    "gstatic.com",
    "recaptcha.net",
    "yandex.ru",
    "yandex.com",
    "yandexcloud.net",
];

/// Не переспрашивать про отложенное обновление и не повторять провалившееся чаще, чем раз в сутки.
const UPDATE_RETRY_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Пути скачиваемых файлов по URL: на macOS событие `Finished` пути не несёт, запоминаем сами.
#[derive(Default)]
struct Downloads(Mutex<HashMap<String, PathBuf>>);

/// Нумерация дополнительных окон (ссылки `_blank` на наш домен).
static EXTRA_WINDOWS: AtomicUsize = AtomicUsize::new(0);

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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Downloads::default())
        .setup(|app| {
            let url: Url = APP_URL.parse().expect("APP_URL должен быть корректным адресом");
            app_window(app.handle(), "main", url).build()?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move { offer_update(handle).await });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("не удалось запустить FinDock");
}

/// Окно с кабинетом: главное и дополнительные устроены одинаково.
fn app_window<'a, R: Runtime>(app: &'a AppHandle<R>, label: &str, url: Url) -> WebviewWindowBuilder<'a, R, AppHandle<R>> {
    let nav_handle = app.clone();
    let new_window_handle = app.clone();

    WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        .title("FinDock")
        .inner_size(1280.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .center()
        .on_navigation(move |url| {
            if stays_in_window(url) {
                return true;
            }
            let _ = nav_handle.opener().open_url(url.as_str(), None::<&str>);
            false
        })
        .on_new_window(move |url, _features| {
            // Наш домен — отдельное окно приложения (та же сессия, есть кнопка закрыть).
            // Чужой — системный браузер: окно без адресной строки и «назад» только запутает.
            if is_own_host(&url) {
                let n = EXTRA_WINDOWS.fetch_add(1, Ordering::Relaxed) + 1;
                match app_window(&new_window_handle, &format!("extra-{n}"), url).build() {
                    Ok(window) => return NewWindowResponse::Create { window },
                    Err(e) => log(&new_window_handle, &format!("не удалось открыть окно: {e}")),
                }
            } else {
                let _ = new_window_handle.opener().open_url(url.as_str(), None::<&str>);
            }
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
}

fn host_matches(host: &str, suffixes: &[&str]) -> bool {
    suffixes
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

/// Хост кабинета (в т.ч. localhost/dev при сборке с `FINDOCK_URL`) или наш домен.
fn is_own_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let app_host = Url::parse(APP_URL).ok().and_then(|u| u.host_str().map(str::to_owned));
    app_host.as_deref() == Some(host) || host_matches(host, OWN_HOST_SUFFIXES)
}

/// Остаётся ли навигация внутри окна: наш домен, страницы входа провайдеров и их ресурсы.
fn stays_in_window(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true; // about:blank, blob:, data: — служебное
    };
    is_own_host(url) || AUTH_HOSTS.contains(&host) || host_matches(host, AUTH_RESOURCE_SUFFIXES)
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

// ---------------------------------------------------------------------------------------------
// Обновление оболочки
// ---------------------------------------------------------------------------------------------

/// Память об обновлениях между запусками: чтобы не дёргать человека и не перекачивать зря.
#[derive(Default, Serialize, Deserialize)]
struct UpdateState {
    /// Версия, которую отложили кнопкой «Позже», и когда.
    postponed_version: Option<String>,
    postponed_at: Option<u64>,
    /// Версия, установка которой провалилась, и когда.
    failed_version: Option<String>,
    failed_at: Option<u64>,
}

impl UpdateState {
    fn path<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
        app.path().app_data_dir().ok().map(|d| d.join("updater.json"))
    }

    fn load<R: Runtime>(app: &AppHandle<R>) -> Self {
        Self::path(app)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save<R: Runtime>(&self, app: &AppHandle<R>) {
        if let Some(p) = Self::path(app) {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_vec_pretty(self) {
                let _ = std::fs::write(p, json);
            }
        }
    }

    /// Эту версию недавно откладывали или не смогли поставить — сегодня не трогаем.
    fn should_skip(&self, version: &str) -> bool {
        let recent = |at: Option<u64>| at.is_some_and(|t| now().saturating_sub(t) < UPDATE_RETRY_AFTER.as_secs());
        (self.postponed_version.as_deref() == Some(version) && recent(self.postponed_at))
            || (self.failed_version.as_deref() == Some(version) && recent(self.failed_at))
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Журнал обновлений: единственное место, где видно, почему обновление не встало.
fn log<R: Runtime>(app: &AppHandle<R>, message: &str) {
    let Ok(dir) = app.path().app_log_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("updater.log")) {
        let _ = writeln!(f, "{} v{} {}", now(), env!("CARGO_PKG_VERSION"), message);
    }
}

/// Тихо скачать обновление, потом спросить. Перезапуск — только по кнопке «Перезапустить»,
/// чтобы не закрыть окно посреди заполнения формы. Любая ошибка (нет сети, GitHub недоступен) —
/// просто нет обновления: кабинет внутри окна от версии оболочки не зависит.
async fn offer_update<R: Runtime>(app: AppHandle<R>) {
    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => return log(&app, &format!("updater недоступен: {e}")),
    };
    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => return,
        Err(e) => return log(&app, &format!("проверка не удалась: {e}")),
    };
    let version = update.version.clone();
    let mut state = UpdateState::load(&app);
    if state.should_skip(&version) {
        return;
    }

    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(b) => b,
        Err(e) => {
            log(&app, &format!("загрузка {version} не удалась: {e}"));
            state.failed_version = Some(version);
            state.failed_at = Some(now());
            return state.save(&app);
        }
    };

    let dialog_app = app.clone();
    app.dialog()
        .message(format!(
            "Готова новая версия FinDock ({version}). Перезапустить приложение сейчас? Это займёт несколько секунд."
        ))
        .title("Обновление FinDock")
        .kind(MessageDialogKind::Info)
        .buttons(MessageDialogButtons::OkCancelCustom("Перезапустить".into(), "Позже".into()))
        .show(move |restart_now| {
            let app = dialog_app;
            let mut state = UpdateState::load(&app);
            if !restart_now {
                state.postponed_version = Some(version);
                state.postponed_at = Some(now());
                return state.save(&app);
            }
            // Windows: install() сам запускает установщик и завершает приложение, оно перезапустится.
            match update.install(bytes) {
                Ok(()) => {
                    log(&app, &format!("установлена {version}, перезапуск"));
                    app.restart();
                }
                Err(e) => {
                    log(&app, &format!("установка {version} не удалась: {e}"));
                    state.failed_version = Some(version);
                    state.failed_at = Some(now());
                    state.save(&app);
                }
            }
        });
}
