//! Что программа умеет делать с системой — и чем каждое действие вернуть.
//!
//! Главное правило здесь одно: **нет отката — нет действия**. Поэтому
//! действие хранится не кодом, а данными: одно и то же описание умеет
//! сказать, как сейчас в системе (`state`), снять прежнее значение
//! (`snapshot`), применить (`apply`) и вернуть ровно то, что было
//! (`revert`). Напиши это четырьмя функциями на каждую настройку — и они
//! разойдутся: откат начнёт возвращать «наверное, было так».
//!
//! Исключение одно и оно отмечено в каталоге явно: удаление встроенных
//! приложений. Вернуть удалённый пакет своими силами нельзя, и программа об
//! этом говорит заранее, а не после.

use crate::reg;
use crate::sys::{powershell, run, run_lossy};

/// Как сейчас в системе.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Стоит ровно то, что записало бы это действие.
    On,
    /// Стоит что-то другое.
    Off,
    /// Проверить нечем. Так у удаления приложений и у команд без пробы —
    /// лучше честное «неизвестно», чем нарисованный переключатель, который
    /// врёт в обе стороны.
    Unknown,
}

/// Когда изменение реально вступает в силу.
///
/// Поле не косметическое. Самая частая жалоба на любой «оптимизатор» —
/// «нажал, ничего не изменилось»: половина настроек внешнего вида читается
/// Проводником при запуске, часть системой — при входе, часть — при
/// загрузке. Если этого не сказать, человек нажмёт ещё раз, уже обратно.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Immediate,
    ExplorerRestart,
    SignOut,
    Reboot,
}

/// Чем человек рискует.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Обратимо и ничего не ломает.
    Safe,
    /// Обратимо, но что-то перестанет работать, и человек должен знать что
    /// именно: поиск, печать, Xbox.
    Caution,
    /// Автоматического отката нет.
    Irreversible,
}

/// Одно значение реестра в составе группы.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RegItem {
    pub name: String,
    pub kind: String,
    pub data: String,
}

/// Чем проверить состояние команды, у которой нет своего способа спросить.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RegProbe {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub data: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Action {
    /// Одно значение реестра.
    Reg {
        path: String,
        name: String,
        kind: String,
        data: String,
    },
    /// Несколько значений одного раздела — как одно неделимое изменение.
    ///
    /// Так устроено, например, ускорение мыши: оно выключено, только когда
    /// `MouseSpeed`, `MouseThreshold1` и `MouseThreshold2` разом равны нулю.
    /// Разложи их на три переключателя — и ни один по отдельности не
    /// изменит ничего.
    RegGroup { path: String, values: Vec<RegItem> },
    /// Значения — в каждый подраздел базового пути.
    ///
    /// Ради сети: `TcpAckFrequency` и `TCPNoDelay` Windows читает не из
    /// общей ветки, а из подраздела конкретного сетевого интерфейса, и GUID
    /// у каждого адаптера свой на каждой машине. Отсюда и вся правда про
    /// «отключить алгоритм Нейгла одной строчкой в реестре»: она не делает
    /// ничего, потому что читают это не оттуда.
    ///
    /// Значений список по той же причине, что и у группы: эти два работают
    /// только вместе.
    RegEach { base: String, values: Vec<RegItem> },
    /// Убрать значение. Для автозагрузки: отключить её — это удалить запись,
    /// а снимок помнит, что в ней было.
    RegRemove { path: String, name: String },
    /// Раздел с пустым значением по умолчанию — заглушка вместо обработчика.
    RegKeyBlank { path: String },
    /// Схема электропитания целиком.
    PowerScheme { guid: String, duplicate: bool },
    /// Один параметр внутри активной схемы питания.
    PowerSetting {
        sub: String,
        setting: String,
        value: String,
    },
    /// Тип запуска службы: `auto`, `demand` или `disabled`.
    Service { name: String, start: String },
    /// Задание планировщика — выключить.
    ///
    /// Путей список, по порядку правдоподобия: Microsoft переименовывает
    /// задания между сборками. На Windows 11 26300 «Microsoft Compatibility
    /// Appraiser» зовётся «…Appraiser Exp», а на прошлых — без приписки.
    /// Спрашивать такое у человека бессмысленно — он этого не знает;
    /// поэтому смотрим, что есть в системе.
    ScheduledTask { paths: Vec<String> },
    /// Команда и обратная к ней. Запасной выход для того, что не ложится в
    /// реестр: `powercfg /hibernate`, `fsutil`.
    Command {
        apply: Vec<String>,
        revert: Vec<String>,
        #[serde(default)]
        probe: Option<RegProbe>,
    },
    /// Удаление встроенного приложения. Единственное необратимое действие.
    AppxRemove { package: String },
}

/// Состояние системы до изменения — всё, что нужно для точного отката.
///
/// Откат идёт именно к снятому значению, а не к «значению по умолчанию»:
/// иначе программа, покрутив настройку, оставила бы человеку не его
/// собственную, а свою догадку о ней.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Snapshot {
    RegValue { kind: String, data: String },
    RegAbsent,
    /// Прежние значения сразу нескольких мест — для групп и для подразделов.
    /// С полным путём каждого: подразделы на каждой машине свои, и
    /// восстановить их по одному описанию действия нельзя.
    RegMany { entries: Vec<RegWas> },
    RegKeyAbsent,
    RegKeyPresent,
    PowerScheme { guid: String },
    PowerIndex { ac: String, dc: String },
    Service { start: String, running: bool },
    /// С путём: имя задания нашлось перебором, и откат обязан идти к тому
    /// же самому, а не искать заново — список кандидатов в новой версии
    /// каталога может оказаться другим.
    Task { path: String, enabled: bool },
    /// У команды снимать нечего — обратная команда известна заранее.
    Command,
    /// Пакет был на месте. Откату не поможет, но в списке применённого
    /// должно быть видно, что именно снесли.
    Appx { package: String },
    None,
}

/// Что стояло в одном конкретном месте реестра до нас.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RegWas {
    pub path: String,
    pub name: String,
    /// `None` — значения не было вовсе; откат его удалит, а не обнулит.
    pub was: Option<reg::Value>,
}

impl Action {
    /// Как сейчас в системе. Ничего не меняет и прав не требует.
    pub fn state(&self) -> State {
        match self {
            Action::Reg {
                path,
                name,
                kind,
                data,
            } => match reg::query(path, name) {
                Some(v) => on_off(reg::same_value(kind, data, &v)),
                None => State::Off,
            },
            Action::RegGroup { path, values } => {
                let all = values.iter().all(|v| {
                    reg::query(path, &v.name)
                        .map(|got| reg::same_value(&v.kind, &v.data, &got))
                        .unwrap_or(false)
                });
                on_off(all)
            }
            Action::RegEach { base, values } => {
                let subs = reg::subkeys(base);
                if subs.is_empty() {
                    return State::Unknown;
                }
                on_off(subs.iter().all(|sub| {
                    values.iter().all(|v| {
                        reg::query(sub, &v.name)
                            .map(|got| reg::same_value(&v.kind, &v.data, &got))
                            .unwrap_or(false)
                    })
                }))
            }
            Action::RegRemove { path, name } => on_off(reg::query(path, name).is_none()),
            Action::RegKeyBlank { path } => on_off(reg::key_exists(path)),
            Action::PowerScheme { guid, .. } => match active_scheme() {
                Some(active) => on_off(same_scheme(&active, guid)),
                None => State::Unknown,
            },
            Action::PowerSetting {
                sub,
                setting,
                value,
            } => match power_index(sub, setting) {
                Some((ac, _)) => match (reg::parse_number(&ac), reg::parse_number(value)) {
                    (Some(a), Some(b)) => on_off(a == b),
                    _ => State::Unknown,
                },
                None => State::Unknown,
            },
            Action::Service { name, start } => match service_start(name) {
                Some(current) => on_off(current.eq_ignore_ascii_case(start)),
                None => State::Unknown,
            },
            Action::ScheduledTask { paths } => match existing_task(paths) {
                // Действие выключает задание, поэтому «включено» для нас —
                // это выключенное задание.
                Some((_, enabled)) => on_off(!enabled),
                // Задания нет в этой сборке Windows — выключать нечего, и
                // показать это надо как «неизвестно», а не как «выключено»:
                // иначе переключатель предложит применить то, чего нет.
                None => State::Unknown,
            },
            Action::Command { probe, .. } => match probe {
                Some(p) => match reg::query(&p.path, &p.name) {
                    Some(v) => on_off(reg::same_value(&p.kind, &p.data, &v)),
                    None => State::Off,
                },
                None => State::Unknown,
            },
            Action::AppxRemove { package } => match appx_present(package) {
                Some(present) => on_off(!present),
                None => State::Unknown,
            },
        }
    }

    /// Снимает состояние «до». Зовётся строго перед `apply`.
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        match self {
            Action::Reg { path, name, .. } => Ok(match reg::query(path, name) {
                Some(v) => Snapshot::RegValue {
                    kind: v.kind,
                    data: v.data,
                },
                None => Snapshot::RegAbsent,
            }),
            Action::RegGroup { path, values } => Ok(Snapshot::RegMany {
                entries: values
                    .iter()
                    .map(|v| RegWas {
                        path: path.clone(),
                        name: v.name.clone(),
                        was: reg::query(path, &v.name),
                    })
                    .collect(),
            }),
            Action::RegEach { base, values } => {
                let subs = reg::subkeys(base);
                if subs.is_empty() {
                    return Err(format!("в {base} нет ни одного подраздела"));
                }
                Ok(Snapshot::RegMany {
                    entries: subs
                        .iter()
                        .flat_map(|sub| {
                            values.iter().map(move |v| RegWas {
                                path: sub.clone(),
                                name: v.name.clone(),
                                was: reg::query(sub, &v.name),
                            })
                        })
                        .collect(),
                })
            }
            Action::RegRemove { path, name } => Ok(match reg::query(path, name) {
                Some(v) => Snapshot::RegValue {
                    kind: v.kind,
                    data: v.data,
                },
                None => Snapshot::RegAbsent,
            }),
            Action::RegKeyBlank { path } => Ok(if reg::key_exists(path) {
                Snapshot::RegKeyPresent
            } else {
                Snapshot::RegKeyAbsent
            }),
            Action::PowerScheme { .. } => Ok(Snapshot::PowerScheme {
                guid: active_scheme().ok_or("не удалось узнать активную схему питания")?,
            }),
            Action::PowerSetting { sub, setting, .. } => {
                let (ac, dc) = power_index(sub, setting).ok_or_else(|| {
                    // Параметра может не быть вовсе: набор параметров схемы
                    // зависит и от схемы, и от драйвера процессора. На Ryzen
                    // с CPPC «Агрессивный режим ускорения» в схеме
                    // «Максимальная производительность» отсутствует — и это
                    // надо сказать словами, а не кодом ошибки powercfg.
                    format!(
                        "этого параметра нет в активной схеме питания: его не поддерживает \
                         либо сама схема, либо драйвер процессора ({setting})"
                    )
                })?;
                Ok(Snapshot::PowerIndex { ac, dc })
            }
            Action::Service { name, .. } => {
                let start =
                    service_start(name).ok_or(format!("службы {name} нет в этой системе"))?;
                Ok(Snapshot::Service {
                    start,
                    running: service_running(name),
                })
            }
            Action::ScheduledTask { paths } => {
                let (path, enabled) = existing_task(paths).ok_or_else(|| {
                    format!("в этой системе нет ни одного из заданий: {}", paths.join(", "))
                })?;
                Ok(Snapshot::Task { path, enabled })
            }
            Action::Command { .. } => Ok(Snapshot::Command),
            Action::AppxRemove { package } => Ok(Snapshot::Appx {
                package: package.clone(),
            }),
        }
    }

    pub fn apply(&self) -> Result<(), String> {
        match self {
            Action::Reg {
                path,
                name,
                kind,
                data,
            } => reg::set(path, name, kind, data),
            Action::RegGroup { path, values } => {
                for v in values {
                    reg::set(path, &v.name, &v.kind, &v.data)?;
                }
                Ok(())
            }
            Action::RegEach { base, values } => {
                let subs = reg::subkeys(base);
                if subs.is_empty() {
                    return Err(format!("в {base} нет ни одного подраздела"));
                }
                for sub in subs {
                    for v in values {
                        reg::set(&sub, &v.name, &v.kind, &v.data)?;
                    }
                }
                Ok(())
            }
            // Значения может уже не быть — цель достигнута и без нас.
            Action::RegRemove { path, name } => {
                if reg::query(path, name).is_none() {
                    return Ok(());
                }
                reg::delete_value(path, name)
            }
            Action::RegKeyBlank { path } => reg::create_blank_key(path),
            Action::PowerScheme { guid, duplicate } => {
                // «Максимальная производительность» в системе есть, но
                // скрыта: пока её не «размножили», `setactive` её не найдёт.
                //
                // Размножаем только если схемы ещё нет в списке. Иначе
                // каждое нажатие добавляло бы в список ещё одну копию — на
                // машине, где это уже делали другие программы, их
                // накапливаются десятки, и человек потом не может найти
                // среди них свою.
                let listed = scheme_listed(guid);
                let target = if *duplicate && !listed {
                    let out = run("powercfg", &["/duplicatescheme", guid])?;
                    find_guid(&out).unwrap_or_else(|| guid.clone())
                } else {
                    guid.clone()
                };
                run("powercfg", &["/setactive", &target]).map(|_| ())
            }
            Action::PowerSetting {
                sub,
                setting,
                value,
            } => set_power_index(sub, setting, value, value)
                .map_err(|e| explain_power_error(&e, sub, setting, value)),
            Action::Service { name, start } => {
                set_service_start(name, start)?;
                // Выключенная, но ещё работающая служба — это настройка,
                // которая вступит в силу только после перезагрузки. Человек
                // при этом видит «применено».
                if start.eq_ignore_ascii_case("disabled") {
                    let _ = run("sc", &["stop", name]);
                }
                Ok(())
            }
            Action::ScheduledTask { paths } => {
                let (path, _) = existing_task(paths).ok_or_else(|| {
                    format!("в этой системе нет ни одного из заданий: {}", paths.join(", "))
                })?;
                set_task(&path, false)
            }
            Action::Command { apply, .. } => run_line(apply),
            Action::AppxRemove { package } => powershell(&format!(
                "Get-AppxPackage -Name '{}' | Remove-AppxPackage",
                ps_quote(package)
            ))
            .map(|_| ()),
        }
    }

    /// Возвращает систему к снятому состоянию.
    ///
    /// Снимок не подходящего вида — это не «ничего не делаем», а ошибка с
    /// внятным текстом: такой снимок мог остаться от другой версии каталога,
    /// и молчаливое «готово» оставило бы человека с нашей настройкой и
    /// уверенностью, что её сняли.
    pub fn revert(&self, snap: &Snapshot) -> Result<(), String> {
        match (self, snap) {
            (
                Action::Reg { path, name, .. } | Action::RegRemove { path, name },
                Snapshot::RegValue { kind, data },
            ) => reg::set(path, name, kind, data),
            (
                Action::Reg { path, name, .. } | Action::RegRemove { path, name },
                Snapshot::RegAbsent,
            ) => {
                // Своё значение удаляем; если его уже нет, цель достигнута.
                let _ = reg::delete_value(path, name);
                Ok(())
            }
            (Action::RegGroup { .. } | Action::RegEach { .. }, Snapshot::RegMany { entries }) => {
                for e in entries {
                    match &e.was {
                        Some(v) => reg::set(&e.path, &e.name, &v.kind, &v.data)?,
                        None => {
                            // Значения не было — удаляем своё. Если его уже
                            // нет, это не ошибка: цель достигнута.
                            let _ = reg::delete_value(&e.path, &e.name);
                        }
                    }
                }
                Ok(())
            }
            (Action::RegKeyBlank { path }, Snapshot::RegKeyAbsent) => reg::delete_key(path),
            (Action::RegKeyBlank { .. }, Snapshot::RegKeyPresent) => Ok(()),
            (Action::PowerScheme { .. }, Snapshot::PowerScheme { guid }) => {
                run("powercfg", &["/setactive", guid]).map(|_| ())
            }
            (
                Action::PowerSetting { sub, setting, .. },
                Snapshot::PowerIndex { ac, dc },
            ) => set_power_index(sub, setting, ac, dc),
            (Action::Service { name, .. }, Snapshot::Service { start, running }) => {
                set_service_start(name, start)?;
                if *running && !service_running(name) {
                    let _ = run("sc", &["start", name]);
                }
                Ok(())
            }
            (Action::ScheduledTask { .. }, Snapshot::Task { path, enabled }) => {
                if !enabled {
                    // Задание и до нас было выключено — включать его обратно
                    // значило бы вернуть не как было.
                    return Ok(());
                }
                set_task(path, true)
            }
            (Action::Command { revert, .. }, Snapshot::Command) => run_line(revert),
            (Action::AppxRemove { package }, _) => Err(format!(
                "Пакет {package} удалён. Автоматически его не вернуть: \
                 установите приложение заново из Microsoft Store."
            )),
            _ => Err("снимок не подходит к этой настройке: он снят другой версией \
                      каталога. Вернуть прежнее значение автоматически нельзя."
                .into()),
        }
    }
}

fn on_off(yes: bool) -> State {
    if yes {
        State::On
    } else {
        State::Off
    }
}

/// Строка команды как список: первое слово — программа, остальное — аргументы.
fn run_line(line: &[String]) -> Result<(), String> {
    let Some((program, args)) = line.split_first() else {
        return Err("пустая команда в каталоге".into());
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    run(program, &args).map(|_| ())
}

/// Одинарные кавычки внутри строки для PowerShell удваиваются. Без этого
/// имя задания с апострофом закрыло бы строку и остаток ушёл бы в код.
fn ps_quote(text: &str) -> String {
    text.replace('\'', "''")
}

/// Полный путь задания — в папку и имя: `Get-ScheduledTask` хочет их по
/// отдельности.
pub fn split_task(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches('\\');
    match trimmed.rsplit_once('\\') {
        Some((folder, name)) => (format!("{folder}\\"), name.to_string()),
        None => ("\\".to_string(), trimmed.to_string()),
    }
}

// ——— электропитание ———

pub fn active_scheme() -> Option<String> {
    find_guid(&run_lossy("powercfg", &["/getactivescheme"]))
}

/// Есть ли схема в списке `powercfg /list`.
fn scheme_listed(guid: &str) -> bool {
    run_lossy("powercfg", &["/list"])
        .to_lowercase()
        .contains(&guid.to_lowercase())
}

/// Одна и та же схема — или её копия.
///
/// Сравнение не только по GUID, потому что копия схемы получает свой
/// собственный. На машине, где «Максимальную производительность» однажды
/// включила другая программа, активна её копия: по GUID это другая схема, а
/// по сути — та же самая, и писать человеку «выключено» было бы неправдой.
fn same_scheme(active: &str, wanted: &str) -> bool {
    if active.eq_ignore_ascii_case(wanted) {
        return true;
    }
    match (scheme_name(active), scheme_name(wanted)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Название схемы: `powercfg /query` печатает его в скобках первой строкой.
fn scheme_name(guid: &str) -> Option<String> {
    parse_scheme_name(&run_lossy("powercfg", &["/query", guid]))
}

/// Название в скобках из первой строки вывода.
///
/// Берём от первой открывающей скобки до последней закрывающей: у копий,
/// сделанных чужими программами, название бывает со своими скобками —
/// «Ultimate Performance (ExitLag)».
pub fn parse_scheme_name(out: &str) -> Option<String> {
    let line = out.lines().find(|l| l.contains('('))?;
    let start = line.find('(')? + 1;
    let end = line.rfind(')')?;
    if end <= start {
        return None;
    }
    Some(line[start..end].trim().to_string())
}

/// GUID из вывода `powercfg`. Ищется по виду, а не по подписи: подписи
/// переведены на язык системы, а 8-4-4-4-12 выглядит одинаково везде.
pub fn find_guid(text: &str) -> Option<String> {
    text.split(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
        .find(|token| looks_like_guid(token))
        .map(|t| t.to_lowercase())
}

fn looks_like_guid(token: &str) -> bool {
    let parts: Vec<&str> = token.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    parts.len() == 5
        && parts
            .iter()
            .zip(lens)
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Текущее значение параметра схемы питания: от сети и от батареи.
pub fn power_index(sub: &str, setting: &str) -> Option<(String, String)> {
    parse_power_index(&run_lossy(
        "powercfg",
        &["/query", "SCHEME_CURRENT", sub, setting],
    ))
}

/// Разбор вывода `powercfg /query`.
///
/// Берём два последних шестнадцатеричных числа: по порядку это индекс от
/// сети и индекс от батареи. По подписям искать нельзя — они переведены, и
/// на русской системе разбор по словам «Current AC Power Setting Index» не
/// нашёл бы ничего.
pub fn parse_power_index(out: &str) -> Option<(String, String)> {
    let values: Vec<String> = out
        .split_whitespace()
        .filter(|t| t.starts_with("0x") && t.len() == 10)
        .map(|t| t.to_string())
        .collect();
    let mut tail = values.into_iter().rev();
    let dc = tail.next()?;
    let ac = tail.next()?;
    Some((ac, dc))
}

/// Переводит отказ `powercfg` в объяснение.
///
/// «The value specified is malformed, or is not within the range of the
/// target power setting» — настоящий ответ Windows на попытку запретить
/// парковку ядер на Ryzen под схемой «Максимальная производительность»: там
/// этот параметр заблокирован, и допустимого значения у него нет вообще.
/// Человеку надо сказать именно это, а не пересказать английскую строку.
fn explain_power_error(error: &str, sub: &str, setting: &str, value: &str) -> String {
    let ranged = error.contains("range") || error.contains("malformed");
    if !ranged {
        return error.to_string();
    }
    let limits = parse_power_limits(&run_lossy(
        "powercfg",
        &["/query", "SCHEME_CURRENT", sub, setting],
    ));
    match limits {
        Some((min, max)) if reg::parse_number(&min) > reg::parse_number(&max) => format!(
            "Windows не даёт менять этот параметр в текущей схеме питания: он сообщает \
             бессмысленные границы ({min}…{max}), то есть параметр заблокирован. \
             Так бывает на процессорах AMD с CPPC и на схеме «Максимальная \
             производительность» — она и так не паркует ядра."
        ),
        Some((min, max)) => format!(
            "Windows не принимает значение {value}: для этого параметра он допускает \
             от {min} до {max}."
        ),
        None => format!("Windows не принял значение {value} для этого параметра."),
    }
}

/// Границы параметра: первые два шестнадцатеричных числа в выводе — это
/// минимально и максимально возможное значение, в этом порядке.
pub fn parse_power_limits(out: &str) -> Option<(String, String)> {
    let mut values = out
        .split_whitespace()
        .filter(|t| t.starts_with("0x") && t.len() == 10);
    Some((values.next()?.to_string(), values.next()?.to_string()))
}

/// Ставит параметр и от сети, и от батареи.
///
/// От батареи — не ради ноутбуков вообще, а потому что иначе на ноутбуке
/// настройка «работает, пока воткнут провод», и человек видит разное
/// поведение в одной и той же схеме.
fn set_power_index(sub: &str, setting: &str, ac: &str, dc: &str) -> Result<(), String> {
    run(
        "powercfg",
        &["/setacvalueindex", "SCHEME_CURRENT", sub, setting, ac],
    )?;
    run(
        "powercfg",
        &["/setdcvalueindex", "SCHEME_CURRENT", sub, setting, dc],
    )?;
    // Без переактивации схемы изменённые индексы не вступают в силу.
    if let Some(active) = active_scheme() {
        let _ = run("powercfg", &["/setactive", &active]);
    }
    Ok(())
}

// ——— службы ———

/// Тип запуска службы: `auto`, `demand`, `disabled` или `boot`/`system`.
///
/// Разбор цепляется за английские константы `AUTO_START`, `DEMAND_START`,
/// `DISABLED`: подписи `sc` переведены на язык системы, а константы — нет.
pub fn service_start(name: &str) -> Option<String> {
    parse_service_start(&run_lossy("sc", &["qc", name]))
}

pub fn parse_service_start(out: &str) -> Option<String> {
    for (token, label) in [
        ("AUTO_START", "auto"),
        ("DEMAND_START", "demand"),
        ("DISABLED", "disabled"),
        ("BOOT_START", "boot"),
        ("SYSTEM_START", "system"),
    ] {
        if out.contains(token) {
            return Some(label.to_string());
        }
    }
    None
}

pub fn service_running(name: &str) -> bool {
    // «RUNNING» — такая же непереводимая константа, как и типы запуска.
    run_lossy("sc", &["query", name]).contains("RUNNING")
}

fn set_service_start(name: &str, start: &str) -> Result<(), String> {
    // В `sc config` между «start=» и значением обязателен пробел, а знак
    // равенства прилипает к имени параметра. Перепутаешь — получишь
    // невнятное «неверный параметр» вместо настроенной службы.
    run("sc", &["config", name, "start=", start]).map(|_| ())
}

// ——— планировщик ———

/// Первое задание из списка, которое есть в этой системе, и его состояние.
pub fn existing_task(paths: &[String]) -> Option<(String, bool)> {
    paths
        .iter()
        .find_map(|path| task_enabled(path).map(|enabled| (path.clone(), enabled)))
}

fn set_task(path: &str, enabled: bool) -> Result<(), String> {
    let (folder, name) = split_task(path);
    let verb = if enabled { "Enable" } else { "Disable" };
    powershell(&format!(
        "{verb}-ScheduledTask -TaskPath '{}' -TaskName '{}' | Out-Null",
        ps_quote(&folder),
        ps_quote(&name)
    ))
    .map(|_| ())
}

/// Включено ли задание. Через PowerShell, а не `schtasks`: состояние
/// задания в `schtasks` печатается на языке системы, а `.State` — всегда
/// по-английски.
pub fn task_enabled(path: &str) -> Option<bool> {
    let (folder, name) = split_task(path);
    let out = powershell(&format!(
        "(Get-ScheduledTask -TaskPath '{}' -TaskName '{}' -ErrorAction SilentlyContinue).State",
        ps_quote(&folder),
        ps_quote(&name)
    ))
    .ok()?;
    let out = out.trim();
    if out.is_empty() {
        return None;
    }
    Some(!out.eq_ignore_ascii_case("Disabled"))
}

// ——— встроенные приложения ———

pub fn appx_present(package: &str) -> Option<bool> {
    let out = powershell(&format!(
        "if (Get-AppxPackage -Name '{}') {{ 'yes' }} else {{ 'no' }}",
        ps_quote(package)
    ))
    .ok()?;
    match out.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_nahoditsya_v_lyubom_perevode() {
        // Русский вывод powercfg — тот, на котором ломается разбор по словам.
        let ru = "GUID схемы электропитания: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c  (Высокая производительность)";
        let en = "Power Scheme GUID: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c  (High performance)";
        for out in [ru, en] {
            assert_eq!(
                find_guid(out).as_deref(),
                Some("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c")
            );
        }
    }

    #[test]
    fn pohozhee_na_guid_no_ne_guid_ne_beretsya() {
        assert_eq!(find_guid("8c5e7fda-e8bf-4a96-9a85"), None);
        assert_eq!(find_guid("нет здесь никаких guid"), None);
    }

    #[test]
    fn indeksy_pitaniya_berutsya_poslednimi_dvumya() {
        let out = "\r\nПодгруппа GUID: 54533251-82be-4824-96c1-47b60b740d00\r\n  GUID параметра: 893dee8e-2bef-41e0-89c6-b55d0929964c\r\n    Минимально возможное значение: 0x00000000\r\n    Максимально возможное значение: 0x00000064\r\n    Шаг: 0x00000001\r\n  Текущее значение индекса параметра питания (питание от сети): 0x00000005\r\n  Текущее значение индекса параметра питания (питание от батареи): 0x00000063\r\n";
        assert_eq!(
            parse_power_index(out),
            Some(("0x00000005".to_string(), "0x00000063".to_string()))
        );
    }

    #[test]
    fn bez_indeksov_ne_pridumyvaem_znachenie() {
        assert_eq!(parse_power_index("Параметр не найден"), None);
    }

    #[test]
    fn bessmyslennye_granicy_uznayutsya() {
        // Настоящий вывод с машины, где параметр парковки ядер заблокирован:
        // минимум 0x32, максимум 0x00. Такой параметр не выставить никаким
        // значением, и программа должна сказать это, а не перебирать числа.
        let out = "      Minimum Possible Setting: 0x00000032\r\n      Maximum Possible Setting: 0x00000000\r\n      Possible Settings increment: 0x00000001\r\n    Current AC Power Setting Index: 0x00000000\r\n    Current DC Power Setting Index: 0x00000000\r\n";
        let (min, max) = parse_power_limits(out).expect("границы должны прочитаться");
        assert_eq!((min.as_str(), max.as_str()), ("0x00000032", "0x00000000"));
        assert!(reg::parse_number(&min) > reg::parse_number(&max));
    }

    #[test]
    fn tip_zapuska_sluzhby_chitaetsya_na_russkoj_sisteme() {
        let ru = "[SC] QueryServiceConfig: успех\r\n\r\nИМЯ_СЛУЖБЫ: SysMain\r\n        ТИП               : 20  WIN32_SHARE_PROCESS\r\n        ТИП_ЗАПУСКА       : 2   AUTO_START\r\n";
        assert_eq!(parse_service_start(ru).as_deref(), Some("auto"));
        let off = "        ТИП_ЗАПУСКА       : 4   DISABLED";
        assert_eq!(parse_service_start(off).as_deref(), Some("disabled"));
    }

    #[test]
    fn nazvanie_shemy_s_vlozhennymi_skobkami() {
        // Так выглядит копия, сделанная чужой программой: название само со
        // скобками. Режь по первой закрывающей — и получишь «Ultimate
        // Performance (ExitLag», то есть схему, которая ни с чем не сойдётся.
        let out = "Power Scheme GUID: 5707fdc7-4fdf-47a4-9dc1-3a5bb443efcd  (Ultimate Performance (ExitLag))\r\n";
        assert_eq!(
            parse_scheme_name(out).as_deref(),
            Some("Ultimate Performance (ExitLag)")
        );
    }

    #[test]
    fn nazvanie_shemy_na_yazyke_sistemy() {
        let out = "Power Scheme GUID: e9a42b02-d5df-448d-aa00-03f14749eb61  (Максимальна продуктивність)";
        assert_eq!(
            parse_scheme_name(out).as_deref(),
            Some("Максимальна продуктивність")
        );
    }

    #[test]
    fn put_zadaniya_delitsya_na_papku_i_imya() {
        let (folder, name) = split_task("\\Microsoft\\Windows\\Application Experience\\ProgramDataUpdater");
        assert_eq!(folder, "\\Microsoft\\Windows\\Application Experience\\");
        assert_eq!(name, "ProgramDataUpdater");

        // Задание в корне: папка остаётся корнем, а не пустой строкой —
        // Get-ScheduledTask с пустым путём не находит ничего.
        let (folder, name) = split_task("\\OneDriveStandaloneUpdateTask");
        assert_eq!(folder, "\\");
        assert_eq!(name, "OneDriveStandaloneUpdateTask");
    }

    #[test]
    fn kavychki_v_imeni_ne_lomayut_powershell() {
        assert_eq!(ps_quote("O'Brien"), "O''Brien");
    }
}
