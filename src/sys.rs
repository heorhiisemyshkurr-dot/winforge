//! Разговор с Windows: запуск консольных средств системы и чтение их вывода.
//!
//! Программа ничего не делает своими руками — ни драйвера, ни правки файлов
//! системы. Всё, что она умеет, умеет сама Windows: `reg`, `sc`, `powercfg`,
//! `schtasks`, `powershell`. Так получается по двум причинам. Первая: любое
//! изменение можно воспроизвести руками и проверить, что оно сделало. Вторая:
//! у каждого такого средства есть обратная команда, а значит есть откат.
//!
//! Вывод не-Windows собирается заглушками — иначе разбор вывода нельзя было
//! бы проверить тестами нигде, кроме настоящей машины.

#[cfg(target_os = "windows")]
use std::process::Command;
use std::process::Stdio;

/// Команда, которая не мигнёт окном консоли.
///
/// За один обход состояния программа зовёт `reg` несколько десятков раз. Без
/// флага CREATE_NO_WINDOW (0x08000000) экран на это время превращается в
/// мерцание чёрных прямоугольников. Флаг просят при создании процесса —
/// спрятать окно после запуска уже нельзя.
#[cfg(target_os = "windows")]
pub fn quiet(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(program);
    cmd.creation_flags(0x0800_0000);
    cmd
}

/// Запускает средство системы и отдаёт его stdout.
///
/// Неуспешный код возврата — ошибка, и в текст ошибки идёт stderr: там
/// «Отказано в доступе» и «Не удалось найти указанный раздел реестра», то
/// есть ровно то, что человеку надо показать. Один код возврата сам по себе
/// не говорит ничего.
pub fn run(program: &str, args: &[&str]) -> Result<String, String> {
    #[cfg(target_os = "windows")]
    {
        let out = quiet(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("не удалось запустить {program}: {e}"))?;

        if out.status.success() {
            return Ok(decode(&out.stdout));
        }
        let stderr = decode(&out.stderr);
        let stderr = stderr.trim();
        Err(if stderr.is_empty() {
            format!("{program} завершился с кодом {:?}", out.status.code())
        } else {
            format!("{program}: {stderr}")
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = Stdio::null();
        Err(format!(
            "{program} {} — есть только в Windows",
            args.join(" ")
        ))
    }
}

/// То же самое, но без различения успеха и неудачи.
///
/// Нужно там, где «не нашлось» — это обычный ответ, а не поломка: `reg query`
/// по отсутствующему значению выходит с ошибкой, и трактовать её как сбой
/// значило бы ронять обход состояния на первой же ненастроенной машине.
pub fn run_lossy(program: &str, args: &[&str]) -> String {
    run(program, args).unwrap_or_default()
}

pub fn powershell(script: &str) -> Result<String, String> {
    run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
}

/// Консольные средства Windows пишут не в UTF-8, а в кодовой странице
/// консоли — 866 у русской системы. Без перевода сообщение об отказе в
/// доступе доходит до человека мусором.
#[cfg(target_os = "windows")]
fn decode(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    if !text.contains('\u{FFFD}') {
        return text.into_owned();
    }
    raw.iter().map(|&b| cp866(b)).collect()
}

/// Один байт кодовой страницы 866 в символ. Цифры и латиница совпадают с
/// ASCII, кириллица занимает верхнюю половину таблицы с разрывом на
/// псевдографику рамок.
#[cfg(target_os = "windows")]
fn cp866(byte: u8) -> char {
    match byte {
        0x00..=0x7F => byte as char,
        0x80..=0xAF => char::from_u32(0x410 + (byte - 0x80) as u32).unwrap_or('?'),
        0xE0..=0xEF => char::from_u32(0x440 + (byte - 0xE0) as u32).unwrap_or('?'),
        0xF0 => 'Ё',
        0xF1 => 'ё',
        _ => ' ',
    }
}

/// Запущена ли программа с правами администратора.
///
/// Проверяется запуском `fltmc` — он перечисляет фильтры файловой системы и
/// отказывает всем, кроме администратора. Это дешевле обращения к токену
/// процесса и не требует ни одной зависимости; ничего в системе он не меняет.
pub fn is_admin() -> bool {
    #[cfg(target_os = "windows")]
    {
        quiet("fltmc")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// SID текущего пользователя.
///
/// Берётся у `whoami`, а не собирается из имени: SID — единственное, по чему
/// можно назвать профиль человека из процесса, работающего под другой
/// учётной записью.
pub fn current_sid() -> Option<String> {
    let out = run("whoami", &["/user", "/fo", "csv", "/nh"]).ok()?;
    out.split(|c: char| c == ',' || c == '"' || c.is_whitespace())
        .find(|token| token.starts_with("S-1-"))
        .map(str::to_string)
}

/// Перезапускает программу с правами администратора и просит себя закрыться.
///
/// Повысить права у живого процесса нельзя — Windows выдаёт их только при
/// создании. Поэтому единственный честный способ: запросить согласие,
/// запустить второй экземпляр и уйти. Путь к себе берётся у системы, а не
/// собирается из имени: программа может лежать где угодно.
///
/// Вместе с собой программа передаёт два своих: чей профиль считать своим и
/// куда писать список применённого. Без этого у человека без прав
/// администратора Windows поднимет **другую** учётную запись, и обе эти
/// вещи уедут в чужой профиль — проверено на живой машине.
pub fn relaunch_elevated() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("не нашёл собственный файл: {e}"))?
        .display()
        .to_string();
    let mut args = Vec::new();
    args.push(format!("'--store','{}'", crate::store::dir().display()));
    if let Some(sid) = current_sid() {
        args.push(format!("'--as-user','{sid}'"));
    }
    powershell(&format!(
        "Start-Process -FilePath '{}' -ArgumentList {} -Verb RunAs",
        exe.replace('\'', "''"),
        args.join(",")
    ))
    .map(|_| ())
}

/// Перезапускает Проводник.
///
/// Половина настроек внешнего вида лежит в ветках, которые Проводник читает
/// при запуске: записать значение мало, его ещё надо заставить перечитать.
/// Иначе человек нажимает переключатель, ничего не происходит, и он нажимает
/// ещё раз — уже обратно.
pub fn restart_explorer() -> Result<(), String> {
    // taskkill возвращает ошибку, если процесса нет; для нас это не сбой —
    // Проводник всё равно надо поднять.
    let _ = run("taskkill", &["/f", "/im", "explorer.exe"]);
    #[cfg(target_os = "windows")]
    {
        quiet("explorer")
            .stdin(Stdio::null())
            .spawn()
            .map_err(|e| format!("Проводник не запустился: {e}"))?;
        return Ok(());
    }
    #[cfg(not(target_os = "windows"))]
    Err("Проводник есть только в Windows".into())
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    #[test]
    fn kirillica_iz_konsoli_chitaetsya() {
        // «Отказано» в кодовой странице 866.
        let raw = [
            0x8E, 0xE2, 0xAA, 0xA0, 0xA7, 0xA0, 0xAD, 0xAE, // Отказано
        ];
        assert_eq!(super::decode(&raw), "Отказано");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn utf8_ne_perekodiruetsya() {
        assert_eq!(super::decode("HwSchMode    REG_DWORD    0x2".as_bytes()), "HwSchMode    REG_DWORD    0x2");
    }
}
