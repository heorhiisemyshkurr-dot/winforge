//! WinForge — настройка и оптимизация Windows.
//!
//! Программа делает три вещи и ни одной больше: показывает, как сейчас
//! настроена система, меняет настройку по просьбе человека и умеет вернуть
//! её ровно к тому, что было до неё. Третье — не приложение к первым двум, а
//! условие: настройка, которую нельзя снять, в каталог не попадает.
//!
//! Разделение по файлам:
//!
//! | Файл | За что отвечает |
//! |---|---|
//! | `sys.rs` | запуск средств Windows и чтение их вывода |
//! | `reg.rs` | реестр: чтение, запись, сравнение значений |
//! | `action.rs` | действие: состояние, снимок, применение, откат |
//! | `catalog.rs` | вшитый каталог настроек и готовые наборы |
//! | `store.rs` | что применено сейчас и чем это вернуть |
//! | `scan.rs` | автозагрузка, приложения, железо, точка восстановления |
//! | `main.rs` | команды для интерфейса |

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod action;
mod catalog;
mod reg;
mod scan;
mod store;
mod sys;

use action::State;
use std::collections::HashMap;

/// Сколько настроек опрашивается одновременно.
///
/// Опрос состояния — это десятки запусков `reg`, `sc` и PowerShell, по
/// десятки миллисекунд каждый, а у заданий планировщика — по полсекунды.
/// Подряд это открытие программы на минуту; восемь потоков укладывают его в
/// пару секунд. Больше восьми смысла не даёт: упираемся не в процессор, а в
/// стоимость запуска процесса.
const WORKERS: usize = 8;

#[tauri::command]
fn is_admin() -> bool {
    sys::is_admin()
}

#[tauri::command]
fn elevate() -> Result<(), String> {
    sys::relaunch_elevated()
}

#[tauri::command]
fn catalog() -> Vec<catalog::Tweak> {
    catalog::get().tweaks.clone()
}

#[tauri::command]
fn presets() -> Vec<catalog::Preset> {
    catalog::get().presets.clone()
}

/// Как сейчас настроена система — по всем настройкам каталога сразу.
#[tauri::command]
fn states() -> HashMap<String, State> {
    let tweaks = &catalog::get().tweaks;
    let mut result = HashMap::new();
    let chunk = tweaks.len().div_ceil(WORKERS).max(1);

    std::thread::scope(|scope| {
        let handles: Vec<_> = tweaks
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|t| (t.id.clone(), t.action.state()))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        for handle in handles {
            // Паника в опросе состояния не должна ронять окно: пропущенная
            // настройка покажется как «неизвестно», и это лучше закрытой
            // программы.
            if let Ok(part) = handle.join() {
                result.extend(part);
            }
        }
    });
    result
}

/// Состояние одной настройки — чтобы после нажатия перечитать её, а не
/// опрашивать заново все семьдесят.
#[tauri::command]
fn state(id: String) -> Result<State, String> {
    catalog::find(&id)
        .map(|t| t.action.state())
        .ok_or_else(|| format!("нет такой настройки: {id}"))
}

#[tauri::command]
fn applied() -> Vec<store::Applied> {
    store::load()
}

/// Применяет одну настройку.
///
/// Права проверяются до действия, а не после отказа: `reg add` в HKLM без
/// прав отвечает «Отказано в доступе», и человеку из этого текста неясно,
/// что надо просто перезапустить программу.
#[tauri::command]
fn apply(id: String) -> Result<(), String> {
    let tweak = catalog::find(&id).ok_or_else(|| format!("нет такой настройки: {id}"))?;
    if tweak.needs_admin && !sys::is_admin() {
        return Err(
            "Для этой настройки нужны права администратора — перезапустите программу \
             кнопкой наверху."
                .into(),
        );
    }
    store::apply(tweak)
}

#[tauri::command]
fn revert(id: String) -> Result<(), String> {
    store::revert(&id)
}

/// Что получилось при применении набора.
///
/// Отчёт списком, а не одна ошибка: в наборе полтора десятка настроек, и
/// «не удалось» на одной из них — не повод не применять остальные и не
/// повод промолчать о ней.
#[derive(serde::Serialize)]
struct Report {
    done: Vec<String>,
    failed: Vec<Failure>,
}

#[derive(serde::Serialize)]
struct Failure {
    id: String,
    title: String,
    error: String,
}

#[tauri::command]
fn apply_preset(id: String) -> Result<Report, String> {
    let preset = catalog::get()
        .presets
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("нет такого набора: {id}"))?;

    let mut report = Report {
        done: Vec::new(),
        failed: Vec::new(),
    };
    for tweak_id in &preset.tweaks {
        match apply(tweak_id.clone()) {
            Ok(()) => report.done.push(tweak_id.clone()),
            Err(error) => report.failed.push(Failure {
                id: tweak_id.clone(),
                title: catalog::find(tweak_id)
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| tweak_id.clone()),
                error,
            }),
        }
    }
    Ok(report)
}

#[tauri::command]
fn revert_all() -> Vec<String> {
    store::revert_all()
}

#[tauri::command]
fn restart_explorer() -> Result<(), String> {
    sys::restart_explorer()
}

#[tauri::command]
fn restore_point() -> Result<String, String> {
    if !sys::is_admin() {
        return Err("Точку восстановления создаёт только администратор.".into());
    }
    scan::restore_point()
}

#[tauri::command]
fn startup_items() -> Vec<scan::StartupItem> {
    scan::startup_items()
}

#[tauri::command]
fn startup_toggle(id: String, enabled: bool) -> Result<(), String> {
    scan::startup_toggle(&id, enabled)
}

#[tauri::command]
fn apps() -> Result<Vec<scan::App>, String> {
    scan::apps()
}

#[tauri::command]
fn remove_app(full_name: String) -> Result<(), String> {
    scan::remove_app(&full_name)
}

#[tauri::command]
fn hardware() -> Result<serde_json::Value, String> {
    scan::hardware()
}

#[tauri::command]
fn wallpaper() -> String {
    scan::current_wallpaper()
}

#[tauri::command]
fn set_wallpaper(path: String, style: String) -> Result<(), String> {
    scan::set_wallpaper(&path, &style)
}

/// Режим командной строки: применить набор без окна.
///
/// Нужен затем, что большая часть каталога требует прав администратора, а
/// повысить их можно только при создании процесса. С окном это значит
/// «перезапустите программу»; без окна — одно согласие Windows на весь
/// набор. Применение идёт через тот же `store`, что и из окна, поэтому
/// каждое изменение получает снимок и снимается потом кнопкой «вернуть как
/// было».
///
/// Отчёт пишется в файл, а не в консоль: собранная программа помечена как
/// оконная, и её `stdout` в терминале никто не увидит. Обещать вывод,
/// которого нет, хуже, чем сразу сказать, где отчёт.
fn cli(args: &[String]) -> i32 {
    let mut log = String::new();
    let mut failures = 0;

    match args[0].as_str() {
        "apply" => {
            // Имя набора раскрывается в список настроек; отдельные
            // настройки можно передать и поимённо.
            let mut ids: Vec<String> = Vec::new();
            for arg in &args[1..] {
                match catalog::get().presets.iter().find(|p| &p.id == arg) {
                    Some(preset) => ids.extend(preset.tweaks.clone()),
                    None => ids.push(arg.clone()),
                }
            }
            if ids.is_empty() {
                return report("apply: не сказано, что применять", 2);
            }
            log.push_str(&format!("права администратора: {}\n", sys::is_admin()));
            for id in ids {
                let line = match apply(id.clone()) {
                    Ok(()) => format!("применено  {id}"),
                    Err(e) => {
                        failures += 1;
                        format!("НЕ ВЫШЛО   {id}: {e}")
                    }
                };
                log.push_str(&line);
                log.push('\n');
            }
        }
        "revert" => {
            if args.len() < 2 {
                return report("revert: не сказано, что возвращать", 2);
            }
            for id in &args[1..] {
                let line = match store::revert(id) {
                    Ok(()) => format!("снято      {id}"),
                    Err(e) => {
                        failures += 1;
                        format!("НЕ ВЫШЛО   {id}: {e}")
                    }
                };
                log.push_str(&line);
                log.push('\n');
            }
        }
        "revert-all" => {
            let failed = store::revert_all();
            failures = failed.len();
            log.push_str(&format!("снято, не вышло у {}\n", failed.len()));
            for line in failed {
                log.push_str(&format!("НЕ ВЫШЛО   {line}\n"));
            }
        }
        "status" => {
            let states = states();
            for tweak in &catalog::get().tweaks {
                let mark = match states.get(&tweak.id) {
                    Some(State::On) => "стоит     ",
                    Some(State::Off) => "нет       ",
                    _ => "неизвестно",
                };
                log.push_str(&format!("{mark} {}  {}\n", tweak.id, tweak.title));
            }
        }
        "restore-point" => match scan::restore_point() {
            Ok(result) => log.push_str(&format!("точка восстановления: {result}\n")),
            Err(e) => {
                failures += 1;
                log.push_str(&format!("точка восстановления не создана: {e}\n"));
            }
        },
        other => {
            return report(
                &format!(
                    "неизвестная команда: {other}\n\
                     winforge apply <набор|настройка>…\n\
                     winforge revert <настройка>…\n\
                     winforge revert-all\n\
                     winforge status\n\
                     winforge restore-point"
                ),
                2,
            )
        }
    }

    report(&log, if failures > 0 { 1 } else { 0 })
}

/// Пишет отчёт рядом со списком применённого и отдаёт код возврата.
fn report(text: &str, code: i32) -> i32 {
    let dir = store::dir();
    let _ = std::fs::create_dir_all(&dir);
    // С отметкой UTF-8 в начале файла. Без неё и «Блокнот», и Windows
    // PowerShell читают отчёт как ANSI, и он выходит кракозябрами —
    // проверено на этом же отчёте.
    let _ = std::fs::write(dir.join("last-run.txt"), format!("\u{feff}{text}"));
    println!("{text}");
    code
}

/// Снимает общие параметры, которые годятся и окну, и командной строке.
///
/// Снимает, а не разбирает: после них в списке остаётся только команда,
/// поэтому запуск с одним `--store` открывает окно, как и запуск без
/// аргументов вовсе.
fn take_options(args: &mut Vec<String>) {
    let mut i = 0;
    while i < args.len() {
        let has_value = i + 1 < args.len();
        match args[i].as_str() {
            "--store" if has_value => {
                store::set_dir(args.remove(i + 1).into());
                args.remove(i);
            }
            "--as-user" if has_value => {
                reg::set_user_sid(args.remove(i + 1));
                args.remove(i);
            }
            _ => i += 1,
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    take_options(&mut args);
    if !args.is_empty() {
        std::process::exit(cli(&args));
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            is_admin,
            elevate,
            catalog,
            presets,
            states,
            state,
            applied,
            apply,
            revert,
            apply_preset,
            revert_all,
            restart_explorer,
            restore_point,
            startup_items,
            startup_toggle,
            apps,
            remove_app,
            hardware,
            wallpaper,
            set_wallpaper
        ])
        .run(tauri::generate_context!())
        .expect("окно не открылось");
}
