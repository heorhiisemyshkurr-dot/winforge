//! Что программа держит применённым — и чем это вернуть назад.
//!
//! Файл на диске, а не список в памяти: изменение переживает закрытие окна.
//! Держи мы снимки в памяти — человек применил бы набор, вышел, а наутро
//! «вернуть как было» вернуло бы в никуда: прежнего значения уже никто не
//! знает.
//!
//! Самое дорогое правило файла стоит первым в `apply`: у уже применённой
//! настройки снимок **не** переснимается. Иначе повторное нажатие запишет в
//! «как было» наше собственное значение, и откат перестанет что-либо
//! возвращать — тихо, без ошибки, и человек узнает об этом через месяц.

use crate::action::Snapshot;
use crate::catalog::Tweak;
use std::path::PathBuf;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Applied {
    pub id: String,
    /// Название на момент применения. Каталог меняется между версиями, а
    /// список применённого обязан остаться читаемым.
    pub title: String,
    /// Единственное, чем можно вернуть как было.
    pub snapshot: Snapshot,
    /// Когда применили — секунды эпохи. Без разбора дат в коде: интерфейс
    /// умеет это сам, а тащить зависимость ради одной строки незачем.
    pub at: u64,
}

/// Общая блокировка на файл: интерфейс умеет применять набор, а это десятки
/// изменений подряд. Без неё два потока прочитали бы один и тот же список и
/// второй записал бы его обратно, потеряв снимок первого.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Куда писать список, если он задан снаружи.
///
/// Задаётся при повышении прав: там `LOCALAPPDATA` принадлежит уже другой
/// учётной записи, и список применённого лёг бы в профиль администратора.
/// Человек после этого открывает программу и видит пустой список — вернуть
/// изменения ему нечем, хотя снимки целы и лежат в чужой папке.
static OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_dir(dir: PathBuf) {
    let _ = OVERRIDE.set(dir);
}

pub fn dir() -> PathBuf {
    if let Some(dir) = OVERRIDE.get() {
        return dir.clone();
    }
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("WinForge")
}

fn file() -> PathBuf {
    dir().join("applied.json")
}

/// Читает список. Отсутствующий или испорченный файл — это пустой список, а
/// не ошибка: программа должна открываться и в таком случае, иначе одна
/// битая строка json заперла бы человека снаружи его же настроек.
pub fn load() -> Vec<Applied> {
    std::fs::read_to_string(file())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save(list: &[Applied]) -> Result<(), String> {
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("не создал папку {}: {e}", dir.display()))?;
    let raw = serde_json::to_string_pretty(list).map_err(|e| format!("не собрал список: {e}"))?;
    std::fs::write(file(), raw).map_err(|e| format!("не записал список применённого: {e}"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Применяет настройку и запоминает, к чему её откатывать.
pub fn apply(tweak: &Tweak) -> Result<(), String> {
    let _guard = LOCK.lock().map_err(|_| "список занят")?;
    let mut list = load();

    if list.iter().any(|a| a.id == tweak.id) {
        // Уже применено. Применяем ещё раз молча — состояние могло
        // измениться помимо нас, скажем служба поднялась после
        // перезагрузки, — но снимок не трогаем.
        return tweak.action.apply();
    }

    let snapshot = tweak.action.snapshot()?;
    tweak.action.apply()?;
    list.push(Applied {
        id: tweak.id.clone(),
        title: tweak.title.clone(),
        snapshot,
        at: now(),
    });
    save(&list)
}

/// Возвращает одну настройку к снятому состоянию и убирает её из списка.
///
/// Из списка настройка убирается только после успешного отката: иначе
/// неудачная попытка оставила бы изменение в системе и ничего — в списке,
/// то есть вернуть его было бы уже нечем.
pub fn revert(id: &str) -> Result<(), String> {
    let _guard = LOCK.lock().map_err(|_| "список занят")?;
    let mut list = load();
    let Some(pos) = list.iter().position(|a| a.id == id) else {
        return Err(format!(
            "{id} в списке применённого нет — возвращать нечего"
        ));
    };
    let tweak = crate::catalog::find(id)
        .ok_or_else(|| format!("{id} нет в каталоге этой версии программы"))?;

    tweak.action.revert(&list[pos].snapshot)?;
    list.remove(pos);
    save(&list)
}

/// Снимает всё применённое. Возвращает список того, что не получилось, —
/// одна упрямая настройка не должна останавливать откат остальных.
pub fn revert_all() -> Vec<String> {
    let ids: Vec<String> = load().into_iter().map(|a| a.id).collect();
    let mut failed = Vec::new();
    for id in ids {
        if let Err(e) = revert(&id) {
            failed.push(format!("{id}: {e}"));
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn papka_lezhit_v_localappdata() {
        // Рядом с программой файл держать нельзя: в Program Files у
        // обычного пользователя нет прав на запись, и список применённого
        // не сохранился бы именно там, где программа и установлена.
        let d = dir().display().to_string();
        assert!(d.ends_with("WinForge"), "неожиданная папка: {d}");
    }

    #[test]
    fn spisok_s_bitym_json_eto_pustoj_spisok() {
        let broken: Result<Vec<Applied>, _> = serde_json::from_str("{ это не json");
        assert!(broken.is_err());
        // load() такую ошибку глотает намеренно — проверяем сам разбор,
        // чтобы было видно, что глотается именно она.
    }
}
