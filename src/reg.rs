//! Реестр через `reg.exe`: чтение, запись, удаление и сравнение значений.
//!
//! Отдельный модуль, потому что сравнение значений — не такая простая вещь,
//! какой кажется. `reg query` печатает число как `0x2`, каталог хранит его
//! как `2`, Windows по дороге дописывает нули в `REG_BINARY`, а у `REG_SZ`
//! бывает регистр и крайние пробелы. Сравнивай такое строками — и программа
//! будет показывать «выключено» у настройки, которую сама же включила минуту
//! назад.

use crate::sys::{run, run_lossy};

/// Чей `HKCU` считать своим.
///
/// Нужно вот почему. Если у человека обычная учётная запись без прав, то
/// Windows при повышении прав не повышает его — она **переходит на другую
/// учётную запись**, администраторскую. У того процесса `HKCU` уже чужой, и
/// пользовательские настройки уезжают в профиль администратора: программа
/// говорит «применено», а у человека ничего не меняется. Это проверено на
/// живой машине, где так ушла половина набора.
///
/// Поэтому при повышении прав программа передаёт себе SID того, кто её
/// запустил, и пишет не в `HKCU`, а в `HKU\<SID>` — тот же самый куст, но
/// названный прямо, а не «куст текущего процесса».
static USER_SID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_user_sid(sid: String) {
    let _ = USER_SID.set(sid);
}

/// Путь с учётом того, чей профиль считать своим.
fn resolve(path: &str) -> String {
    match USER_SID.get() {
        Some(sid) => rewrite_hkcu(path, sid),
        None => path.to_string(),
    }
}

/// `HKCU\...` → `HKU\<SID>\...`. Остальные кусты не трогаются.
pub fn rewrite_hkcu(path: &str, sid: &str) -> String {
    for prefix in ["HKCU", "HKEY_CURRENT_USER"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with('\\') {
                return format!("HKU\\{sid}{rest}");
            }
        }
    }
    path.to_string()
}

/// Значение реестра: тип и данные ровно в том виде, в каком их печатает
/// `reg query`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Value {
    pub kind: String,
    pub data: String,
}

/// Читает одно значение. Отсутствие раздела и отсутствие значения — это
/// одинаковое «его там нет», а не две разные ошибки: для отката важно только
/// то, что при возврате своё значение надо удалить, а не на что-то заменить.
pub fn query(path: &str, name: &str) -> Option<Value> {
    parse_query(&run_lossy("reg", &["query", &resolve(path), "/v", name]), name)
}

/// Разбор вывода `reg query ... /v ...`.
pub fn parse_query(out: &str, name: &str) -> Option<Value> {
    out.lines()
        .filter_map(parse_line)
        .find(|(found, _)| found.eq_ignore_ascii_case(name))
        .map(|(_, value)| value)
}

/// Одна строка вывода `reg query` — в имя и значение.
///
/// Делим по типу, а не по пробелам: пробелы бывают и в имени значения
/// («Adobe Updater» в автозагрузке), и в данных (пути с пробелами). Режь по
/// первому пробелу — и половина записей автозагрузки окажется с обрезанными
/// именами, то есть отключить их будет нельзя.
fn parse_line(line: &str) -> Option<(String, Value)> {
    let line = line.trim_end();
    let at = type_at(line)?;
    let name = line[..at].trim();
    if name.is_empty() {
        return None;
    }
    let rest = &line[at..];
    let (kind, data) = match rest.find(char::is_whitespace) {
        Some(space) => (&rest[..space], rest[space..].trim()),
        None => (rest, ""),
    };
    Some((
        name.to_string(),
        Value {
            kind: kind.to_string(),
            data: data.to_string(),
        },
    ))
}

/// Где в строке начинается тип значения.
///
/// Ищем `REG_` после пробела: так подпись типа не спутается с именем
/// значения, которое само начинается на `REG_` — такие встречаются.
fn type_at(line: &str) -> Option<usize> {
    line.char_indices()
        .filter(|(i, _)| *i > 0 && line[..*i].ends_with(|c: char| c.is_whitespace()))
        .map(|(i, _)| i)
        .find(|i| line[*i..].starts_with("REG_"))
}

/// Все значения раздела: имя, тип, данные.
///
/// Нужно для автозагрузки — там заранее неизвестно ни одно имя: что лежит в
/// `Run`, зависит от того, какие программы человек ставил.
pub fn values(path: &str) -> Vec<(String, Value)> {
    parse_values(&run_lossy("reg", &["query", &resolve(path)]))
}

pub fn parse_values(out: &str) -> Vec<(String, Value)> {
    out.lines()
        // Строка значения начинается с отступа; заголовок раздела и пути
        // подразделов стоят у левого края.
        .filter(|l| l.starts_with(' ') || l.starts_with('\t'))
        .filter_map(parse_line)
        .collect()
}

pub fn set(path: &str, name: &str, kind: &str, data: &str) -> Result<(), String> {
    run(
        "reg",
        &[
            "add",
            &resolve(path),
            "/v",
            name,
            "/t",
            kind,
            "/d",
            data,
            "/f",
        ],
    )
    .map(|_| ())
}

pub fn delete_value(path: &str, name: &str) -> Result<(), String> {
    run("reg", &["delete", &resolve(path), "/v", name, "/f"]).map(|_| ())
}

pub fn key_exists(path: &str) -> bool {
    run("reg", &["query", &resolve(path)]).is_ok()
}

/// Создаёт раздел и ставит его значению по умолчанию пустую строку.
///
/// Нужно для возврата старого контекстного меню: Windows 11 проверяет не
/// содержимое ветки обработчика, а само её существование, и пустое значение
/// по умолчанию — способ сказать «обработчика здесь нет».
pub fn create_blank_key(path: &str) -> Result<(), String> {
    run("reg", &["add", &resolve(path), "/ve", "/t", "REG_SZ", "/d", "", "/f"]).map(|_| ())
}

pub fn delete_key(path: &str) -> Result<(), String> {
    run("reg", &["delete", &resolve(path), "/f"]).map(|_| ())
}

/// Полные пути непосредственных подразделов.
///
/// `reg query` печатает и значения раздела, и его подразделы; подразделы —
/// строки, начинающиеся с `HKEY_`. Сам раздел тоже встречается заголовком,
/// поэтому отбрасываем его по длине: у заголовка путь в точности равен
/// запрошенному, у подраздела — длиннее.
pub fn subkeys(base: &str) -> Vec<String> {
    let out = run_lossy("reg", &["query", &resolve(base)]);
    let full = expand_hive(base).to_uppercase();
    out.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("HKEY_"))
        .filter(|l| l.len() > full.len() && l.to_uppercase().starts_with(&full))
        .map(str::to_string)
        .collect()
}

/// Короткое имя куста в полное: `reg query` печатает полное, а каталог
/// хранит короткое.
pub fn expand_hive(path: &str) -> String {
    const PAIRS: &[(&str, &str)] = &[
        ("HKLM", "HKEY_LOCAL_MACHINE"),
        ("HKCU", "HKEY_CURRENT_USER"),
        ("HKCR", "HKEY_CLASSES_ROOT"),
        ("HKU", "HKEY_USERS"),
        ("HKCC", "HKEY_CURRENT_CONFIG"),
    ];
    for (short, long) in PAIRS {
        if let Some(rest) = path.strip_prefix(short) {
            // «HKCUX» — не куст HKCU: без проверки на разделитель путь
            // превратился бы в несуществующий.
            if rest.is_empty() || rest.starts_with('\\') {
                return format!("{long}{rest}");
            }
        }
    }
    path.to_string()
}

/// Стоит ли в реестре уже то, что мы собираемся записать.
///
/// Числа сравниваются числами: `reg query` печатает `0x2`, каталог хранит
/// `2`, и это одно и то же значение. Двоичные данные — без разделителей и
/// регистра. Строки — без крайних пробелов, но с регистром: в путях он
/// значащий.
pub fn same_value(kind: &str, want: &str, got: &Value) -> bool {
    let kind = kind.to_uppercase();
    if !got.kind.eq_ignore_ascii_case(&kind) {
        return false;
    }
    match kind.as_str() {
        "REG_DWORD" | "REG_QWORD" => match (parse_number(want), parse_number(&got.data)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
        "REG_BINARY" => clean_hex(want) == clean_hex(&got.data),
        _ => want.trim() == got.data.trim(),
    }
}

fn clean_hex(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Число из реестра или из каталога: с `0x` — шестнадцатеричное, без
/// приставки — как записано. `ffffffff` без приставки тоже встречается,
/// поэтому последняя попытка — шестнадцатеричная.
pub fn parse_number(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    text.parse::<u64>()
        .ok()
        .or_else(|| u64::from_str_radix(text, 16).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced\r\n    TaskbarAl    REG_DWORD    0x0\r\n    Hidden    REG_DWORD    0x1\r\n\r\n";

    #[test]
    fn znachenie_nahoditsya_po_imeni() {
        let v = parse_query(OUT, "TaskbarAl").expect("TaskbarAl должен найтись");
        assert_eq!(v.kind, "REG_DWORD");
        assert_eq!(v.data, "0x0");
        assert_eq!(parse_query(OUT, "Hidden").unwrap().data, "0x1");
    }

    #[test]
    fn imya_sravnivaetsya_bez_registra_i_celikom() {
        // Каталог может хранить другой регистр, чем реестр; а вот «Hid» —
        // не «Hidden», и подстрочное совпадение нашло бы чужое значение.
        assert!(parse_query(OUT, "taskbaral").is_some());
        assert!(parse_query(OUT, "Hid").is_none());
    }

    #[test]
    fn probely_v_dannyh_ne_teryayutsya() {
        let out = "    Wallpaper    REG_SZ    C:\\Users\\bob\\My Pictures\\a b.jpg";
        assert_eq!(
            parse_query(out, "Wallpaper").unwrap().data,
            "C:\\Users\\bob\\My Pictures\\a b.jpg"
        );
    }

    #[test]
    fn zagolovok_razdela_ne_prinimaetsya_za_znachenie() {
        // В заголовке второе «слово» — не тип значения, и без проверки на
        // REG_ разбор вернул бы путь раздела как данные.
        let out = "HKEY_CURRENT_USER\\Software\\Test\\Advanced";
        assert!(parse_query(out, "HKEY_CURRENT_USER\\Software\\Test\\Advanced").is_none());
    }

    #[test]
    fn imya_s_probelom_ne_obrezaetsya() {
        // Так выглядит настоящая запись автозагрузки. Обрежь имя по первому
        // пробелу — и отключить её будет нечем.
        let out = "    Adobe Updater    REG_SZ    \"C:\\Program Files\\Adobe\\u.exe\" /b";
        let (name, value) = parse_values(out).pop().expect("значение должно найтись");
        assert_eq!(name, "Adobe Updater");
        assert_eq!(value.data, "\"C:\\Program Files\\Adobe\\u.exe\" /b");
    }

    #[test]
    fn vse_znacheniya_razdela_perechislyayutsya() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Run\r\n    OneDrive    REG_SZ    C:\\OneDrive.exe /background\r\n    Steam    REG_SZ    C:\\Steam.exe -silent\r\n";
        let names: Vec<String> = parse_values(out).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["OneDrive", "Steam"]);
    }

    #[test]
    fn pustoe_znachenie_ostaetsya_pustym() {
        // У REG_SZ без данных хвоста нет вовсе — разбор не должен на этом
        // спотыкаться и выдавать None.
        let out = "    Empty    REG_SZ";
        let (name, value) = parse_values(out).pop().expect("значение должно найтись");
        assert_eq!(name, "Empty");
        assert_eq!(value.data, "");
    }

    #[test]
    fn chisla_sravnivayutsya_chislami() {
        let got = Value {
            kind: "REG_DWORD".into(),
            data: "0x2".into(),
        };
        assert!(same_value("REG_DWORD", "2", &got));
        assert!(same_value("reg_dword", "0x00000002", &got));
        assert!(!same_value("REG_DWORD", "3", &got));
    }

    #[test]
    fn ffffffff_bez_pristavki_eto_chislo() {
        // NetworkThrottlingIndex в каталогах принято писать именно так.
        let got = Value {
            kind: "REG_DWORD".into(),
            data: "0xffffffff".into(),
        };
        assert!(same_value("REG_DWORD", "ffffffff", &got));
    }

    #[test]
    fn tip_dolzhen_sovpadat() {
        let got = Value {
            kind: "REG_SZ".into(),
            data: "2".into(),
        };
        assert!(!same_value("REG_DWORD", "2", &got));
    }

    #[test]
    fn dvoichnoe_sravnivaetsya_bez_razdelitelej() {
        let got = Value {
            kind: "REG_BINARY".into(),
            data: "0300000000000000".into(),
        };
        assert!(same_value("REG_BINARY", "03 00 00 00 00 00 00 00", &got));
    }

    #[test]
    fn chuzhoj_profil_ne_poluchaet_nastroek() {
        // Ровно та ошибка, что случилась на живой машине: под правами
        // администратора «своим» оказался чужой профиль, и прозрачность
        // выключилась не у того человека.
        let sid = "S-1-5-21-111-222-333-1001";
        assert_eq!(
            rewrite_hkcu(r"HKCU\Control Panel\Mouse", sid),
            r"HKU\S-1-5-21-111-222-333-1001\Control Panel\Mouse"
        );
        assert_eq!(
            rewrite_hkcu(r"HKEY_CURRENT_USER\Software\X", sid),
            r"HKU\S-1-5-21-111-222-333-1001\Software\X"
        );
        // Машинные ветки не трогаем: у них профиля нет.
        assert_eq!(
            rewrite_hkcu(r"HKLM\SOFTWARE\Policies", sid),
            r"HKLM\SOFTWARE\Policies"
        );
        // И «HKCUX» — не куст HKCU.
        assert_eq!(rewrite_hkcu("HKCUX", sid), "HKCUX");
    }

    #[test]
    fn kust_razvorachivaetsya() {
        assert_eq!(expand_hive("HKCU\\Software"), "HKEY_CURRENT_USER\\Software");
        assert_eq!(expand_hive("HKCUX"), "HKCUX");
    }
}
