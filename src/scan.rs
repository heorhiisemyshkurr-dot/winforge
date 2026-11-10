//! Что на этой машине есть: автозагрузка, встроенные приложения, железо.
//!
//! Всё в этом файле — про конкретный компьютер, а не про каталог. Каталог
//! знает, какие настройки бывают; здесь программа смотрит, что именно стоит
//! у человека, и показывает это списком.

use crate::reg;
use crate::sys::{powershell, run};

// ——— автозагрузка ———

/// Где Windows держит отметку «эта запись автозагрузки включена».
///
/// Отметку ставит диспетчер задач, и программа делает то же самое, а не
/// удаляет запись: удаление сработало бы и было бы обратимо, но человек,
/// открыв диспетчер задач, увидел бы, что программы в списке больше нет
/// вообще — и решил бы, что её снесли.
/// Отметка лежит там же, где и сама запись: у пользовательских — в HKCU, у
/// машинных — в HKLM. Это проверено на живой системе, и ошибка здесь была бы
/// из самых обидных: отметка записалась бы в HKCU, программа сказала бы
/// «готово», а Проводник читал бы машинную запись из HKLM и запускал её
/// по-прежнему.
const APPROVED_RUN: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const APPROVED_RUN_HKLM: &str =
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// Для 32-разрядных программ у Windows отдельный раздел отметок.
const APPROVED_RUN32: &str =
    r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run32";
const APPROVED_FOLDER: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";

const RUN_HKCU: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_HKLM: &str = r"HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const RUN_HKLM32: &str = r"HKLM\SOFTWARE\Wow6432Node\Microsoft\Windows\CurrentVersion\Run";

/// Двенадцать байт: первый — включено или нет, остальные диспетчер задач
/// заполняет временем отключения. Проводник на остальные не смотрит, поэтому
/// нули его устраивают.
const BLOB_ON: &str = "020000000000000000000000";
const BLOB_OFF: &str = "030000000000000000000000";

#[derive(serde::Serialize)]
pub struct StartupItem {
    /// Источник и имя через вертикальную черту — по нему же запись потом
    /// находится при переключении.
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: String,
    pub enabled: bool,
    /// Машинные записи отмечаются в HKLM, а туда без прав не записать.
    pub needs_admin: bool,
}

pub fn startup_items() -> Vec<StartupItem> {
    let mut items = Vec::new();

    for (prefix, path, source) in [
        ("hkcu", RUN_HKCU, "Этот пользователь"),
        ("hklm", RUN_HKLM, "Все пользователи"),
        ("hklm32", RUN_HKLM32, "Все пользователи, 32 бита"),
    ] {
        for (name, value) in reg::values(path) {
            items.push(StartupItem {
                id: format!("{prefix}|{name}"),
                enabled: approved(approved_key(prefix), &name),
                name,
                command: value.data,
                source: source.to_string(),
                needs_admin: prefix != "hkcu",
            });
        }
    }

    for (dir, source) in startup_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().to_string();
            // desktop.ini — настройки оформления папки, а не программа.
            if file.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            items.push(StartupItem {
                id: format!("folder|{file}"),
                enabled: approved(APPROVED_FOLDER, &file),
                name: file,
                command: entry.path().display().to_string(),
                source: source.to_string(),
                needs_admin: false,
            });
        }
    }

    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    items
}

fn startup_dirs() -> Vec<(std::path::PathBuf, &'static str)> {
    let tail = r"Microsoft\Windows\Start Menu\Programs\Startup";
    let mut dirs = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        dirs.push((
            std::path::PathBuf::from(appdata).join(tail),
            "Папка автозагрузки",
        ));
    }
    if let Ok(common) = std::env::var("ProgramData") {
        dirs.push((
            std::path::PathBuf::from(common).join(tail),
            "Папка автозагрузки (все)",
        ));
    }
    dirs
}

/// Включена ли запись. Отметки нет — значит включена: Windows считает так же,
/// отметка появляется только после первого отключения.
fn approved(key: &str, name: &str) -> bool {
    match reg::query(key, name) {
        Some(v) => approved_blob_enabled(&v.data),
        None => true,
    }
}

/// Разбор самой отметки: смотрим только первый байт. Остальные одиннадцать —
/// время отключения, и у отметок, поставленных разными версиями Windows, они
/// разные; сравнивай отметку целиком — и чужая оказалась бы «включённой».
fn approved_blob_enabled(data: &str) -> bool {
    !data.trim_start().to_lowercase().starts_with("03")
}

/// Где лежит отметка для записи из этого источника.
fn approved_key(prefix: &str) -> &'static str {
    match prefix {
        "hklm" => APPROVED_RUN_HKLM,
        "hklm32" => APPROVED_RUN32,
        "folder" => APPROVED_FOLDER,
        _ => APPROVED_RUN,
    }
}

pub fn startup_toggle(id: &str, enabled: bool) -> Result<(), String> {
    let (prefix, name) = id
        .split_once('|')
        .ok_or_else(|| format!("непонятная запись автозагрузки: {id}"))?;
    if !matches!(prefix, "hkcu" | "hklm" | "hklm32" | "folder") {
        return Err(format!("неизвестный источник автозагрузки: {prefix}"));
    }
    if prefix.starts_with("hklm") && !crate::sys::is_admin() {
        return Err(
            "Эта запись общая для всех пользователей, и её отметка лежит в HKLM — \
             нужны права администратора."
                .into(),
        );
    }
    let blob = if enabled { BLOB_ON } else { BLOB_OFF };
    reg::set(approved_key(prefix), name, "REG_BINARY", blob)
}

// ——— встроенные приложения ———

#[derive(serde::Serialize)]
pub struct App {
    pub name: String,
    pub full_name: String,
    pub publisher: String,
}

/// Список установленных приложений Microsoft Store.
///
/// Рамки (`IsFramework`) и системные пакеты (`NonRemovable`) отброшены сразу:
/// предложить человеку снести то, что всё равно не снесётся, значит
/// пообещать и не сделать.
pub fn apps() -> Result<Vec<App>, String> {
    let raw = powershell(
        "Get-AppxPackage | Where-Object { -not $_.IsFramework -and -not $_.NonRemovable } | \
         ForEach-Object { [pscustomobject]@{ name = $_.Name; full = $_.PackageFullName; \
         publisher = $_.Publisher } } | ConvertTo-Json -Compress",
    )?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let parsed: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("не разобрал список приложений: {e}"))?;
    // Один пакет PowerShell отдаёт объектом, а не списком из одного.
    let list = match parsed {
        serde_json::Value::Array(items) => items,
        other => vec![other],
    };
    let mut apps: Vec<App> = list
        .into_iter()
        .filter_map(|v| {
            Some(App {
                name: v.get("name")?.as_str()?.to_string(),
                full_name: v.get("full")?.as_str()?.to_string(),
                publisher: short_publisher(v.get("publisher").and_then(|p| p.as_str())?),
            })
        })
        .collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(apps)
}

/// Из `CN=Microsoft Corporation, O=..., L=Redmond...` — только имя.
pub fn short_publisher(raw: &str) -> String {
    raw.split(',')
        .map(str::trim)
        .find_map(|part| part.strip_prefix("CN="))
        .unwrap_or(raw)
        .to_string()
}

/// Удаляет приложение. Необратимо — поэтому зовётся только из того места
/// интерфейса, где это написано рядом с кнопкой.
pub fn remove_app(full_name: &str) -> Result<(), String> {
    if full_name.contains('\'') || full_name.contains(';') {
        return Err("недопустимое имя пакета".into());
    }
    powershell(&format!(
        "Remove-AppxPackage -Package '{full_name}' -ErrorAction Stop"
    ))
    .map(|_| ())
}

// ——— железо ———

/// Опись машины одним запросом.
///
/// Одним, а не десятью: каждый запуск PowerShell стоит около полусекунды, и
/// пять отдельных вызовов превратили бы открытие программы в ожидание.
pub fn hardware() -> Result<serde_json::Value, String> {
    let raw = powershell(
        "$os = Get-CimInstance Win32_OperatingSystem; \
         $cpu = Get-CimInstance Win32_Processor | Select-Object -First 1; \
         $mem = @(Get-CimInstance Win32_PhysicalMemory); \
         $board = Get-CimInstance Win32_BaseBoard; \
         [pscustomobject]@{ \
           os = $os.Caption; \
           build = $os.BuildNumber; \
           uptime_hours = [math]::Round(((Get-Date) - $os.LastBootUpTime).TotalHours, 1); \
           ram_free_gb = [math]::Round($os.FreePhysicalMemory / 1MB, 1); \
           cpu = $cpu.Name.Trim(); \
           cores = $cpu.NumberOfCores; \
           threads = $cpu.NumberOfLogicalProcessors; \
           board = ($board.Manufacturer + ' ' + $board.Product).Trim(); \
           ram_gb = [math]::Round((($mem | Measure-Object -Property Capacity -Sum).Sum) / 1GB, 1); \
           ram_speed = ($mem | Select-Object -First 1).Speed; \
           ram_sticks = $mem.Count; \
           gpu = @(Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }); \
           disks = @(Get-CimInstance Win32_DiskDrive | ForEach-Object { \
             [pscustomobject]@{ model = $_.Model; size_gb = [math]::Round($_.Size / 1GB, 0) } }) \
         } | ConvertTo-Json -Compress -Depth 4",
    )?;
    serde_json::from_str(raw.trim()).map_err(|e| format!("не разобрал опись машины: {e}"))
}

// ——— точка восстановления ———

/// Создаёт точку восстановления системы.
///
/// Отвечает тремя разными словами, а не «готово», потому что у Windows тут
/// три разных исхода, и два из них молчаливые: защита системы может быть
/// выключена на диске, а при включённой — Windows отказывается делать больше
/// одной точки в сутки и при этом не считает это ошибкой. Сказать «готово» в
/// этих случаях значило бы продать человеку страховку, которой нет.
pub fn restore_point() -> Result<String, String> {
    let out = powershell(
        "$before = @(Get-ComputerRestorePoint).Count; \
         try { Enable-ComputerRestore -Drive \"$env:SystemDrive\\\" -ErrorAction Stop } \
         catch { 'no-protection'; exit }; \
         Checkpoint-Computer -Description 'WinForge' -RestorePointType MODIFY_SETTINGS \
           -ErrorAction SilentlyContinue; \
         if (@(Get-ComputerRestorePoint).Count -gt $before) { 'created' } else { 'skipped' }",
    )?;
    Ok(match out.trim() {
        s if s.ends_with("created") => "created".to_string(),
        s if s.ends_with("skipped") => "skipped".to_string(),
        s if s.ends_with("no-protection") => "no-protection".to_string(),
        other => return Err(format!("неожиданный ответ Windows: {other}")),
    })
}

// ——— обои ———

/// Ставит обои. Значения в реестре плюс просьба к Проводнику перечитать их:
/// без второго шага обои поменяются только после входа в систему.
pub fn set_wallpaper(path: &str, style: &str) -> Result<(), String> {
    if !std::path::Path::new(path).is_file() {
        return Err(format!("файла {path} нет"));
    }
    // 10 — заполнение, 6 — по размеру, 2 — растянуть, 0 — по центру.
    let (wallpaper_style, tile) = match style {
        "fill" => ("10", "0"),
        "fit" => ("6", "0"),
        "stretch" => ("2", "0"),
        "center" => ("0", "0"),
        "tile" => ("0", "1"),
        other => return Err(format!("неизвестное расположение обоев: {other}")),
    };
    const DESKTOP: &str = r"HKCU\Control Panel\Desktop";
    reg::set(DESKTOP, "Wallpaper", "REG_SZ", path)?;
    reg::set(DESKTOP, "WallpaperStyle", "REG_SZ", wallpaper_style)?;
    reg::set(DESKTOP, "TileWallpaper", "REG_SZ", tile)?;
    run(
        "rundll32.exe",
        &["user32.dll,UpdatePerUserSystemParameters", "1", "True"],
    )
    .map(|_| ())
}

/// Какие обои стоят сейчас — чтобы человеку было чем вернуть прежние.
pub fn current_wallpaper() -> String {
    reg::query(r"HKCU\Control Panel\Desktop", "Wallpaper")
        .map(|v| v.data)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otmetka_otklyucheniya_chitaetsya() {
        // Так эту отметку пишет диспетчер задач: 03 — выключено, в хвосте
        // время отключения.
        assert!(!super::approved_blob_enabled("03000000227369033CDADC01"));
        assert!(super::approved_blob_enabled("020000000000000000000000"));
        // 06 Windows ставит сама машинным записям — это тоже «включено».
        // Сравнивай с «02» — и SecurityHealth показался бы выключенным.
        assert!(super::approved_blob_enabled("060000000000000000000000"));
        // Отметки нет — запись включена.
        assert!(super::approved_blob_enabled(""));
    }

    #[test]
    fn otmetka_lezhit_tam_zhe_gde_zapis() {
        // Отметку машинной записи в HKCU Проводник не читает: запись
        // продолжила бы запускаться, а программа сказала бы «готово».
        assert!(super::approved_key("hklm").starts_with("HKLM"));
        assert!(super::approved_key("hklm32").ends_with("Run32"));
        assert!(super::approved_key("hkcu").starts_with("HKCU"));
        assert!(super::approved_key("folder").ends_with("StartupFolder"));
    }

    #[test]
    fn izdatel_sokrashchaetsya_do_imeni() {
        assert_eq!(
            short_publisher("CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond"),
            "Microsoft Corporation"
        );
        // Без CN= оставляем как есть, а не пустую строку.
        assert_eq!(short_publisher("Valve"), "Valve");
    }

    #[test]
    fn imya_paketa_s_kavychkoj_otklonyaetsya() {
        // Имя уходит в строку PowerShell; кавычка в нём — это возможность
        // дописать туда чужую команду.
        assert!(remove_app("App'; rm -rf /; '").is_err());
    }

    #[test]
    fn raspolozhenie_oboev_proveryaetsya() {
        assert!(set_wallpaper("C:\\нет-такого-файла.jpg", "fill").is_err());
    }
}
