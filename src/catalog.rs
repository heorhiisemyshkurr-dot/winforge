//! Каталог настроек: что программа предлагает сделать с системой.
//!
//! Каталог лежит в json рядом с кодом и вшивается в программу при сборке.
//! Это сознательно: содержимое каталога — не секрет, а предмет проверки.
//! Любой может открыть файл и увидеть, в какую ветку реестра пойдёт
//! программа, прежде чем нажать переключатель. Прятать это значило бы
//! просить доверия вместо того, чтобы его заслужить.
//!
//! Разбивка по файлам — по разделам интерфейса, а не по виду действия: когда
//! добавляют настройку, её кладут туда, где человек будет её искать.

use crate::action::{Action, Effect, Risk};

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Внешний вид: панель задач, Проводник, тема, курсор мыши.
    Appearance,
    /// Приватность и предустановленный хлам.
    Privacy,
    /// Автозагрузка, службы, задания планировщика.
    Startup,
    /// Питание, процессор, видеокарта, диск, сеть.
    Performance,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Tweak {
    pub id: String,
    pub category: Category,
    /// Короткое название — то, что человек читает на переключателе.
    pub title: String,
    /// Что это делает и чем за это платят. Поле обязательное: настройка без
    /// объяснения последствий — это предложение нажать вслепую.
    pub detail: String,
    pub effect: Effect,
    pub risk: Risk,
    pub needs_admin: bool,
    /// Имя взаимоисключающей группы. У схем питания она одна на все: включив
    /// вторую, человек не «добавляет» её, а меняет первую, и интерфейс
    /// обязан показывать это переключателем выбора, а не галочками.
    #[serde(default)]
    pub exclusive: Option<String>,
    pub action: Action,
}

/// Готовый набор: несколько настроек, которые осмысленно включать вместе.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Preset {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub tweaks: Vec<String>,
}

pub struct Catalog {
    pub tweaks: Vec<Tweak>,
    pub presets: Vec<Preset>,
}

const FILES: &[&str] = &[
    include_str!("../catalog/appearance.json"),
    include_str!("../catalog/privacy.json"),
    include_str!("../catalog/startup.json"),
    include_str!("../catalog/performance.json"),
];

const PRESETS: &str = include_str!("../catalog/presets.json");

/// Разбирает вшитый каталог. Ошибка здесь невозможна у собранной программы:
/// файлы те же, что проверяет тест `katalog_razbiraetsya`, — но разбор
/// отдаёт `Result`, чтобы эта невозможность проверялась, а не
/// предполагалась.
pub fn parse() -> Result<Catalog, String> {
    let mut tweaks = Vec::new();
    for raw in FILES {
        let part: Vec<Tweak> =
            serde_json::from_str(raw).map_err(|e| format!("каталог не разобран: {e}"))?;
        tweaks.extend(part);
    }
    let presets: Vec<Preset> =
        serde_json::from_str(PRESETS).map_err(|e| format!("наборы не разобраны: {e}"))?;
    Ok(Catalog { tweaks, presets })
}

static LOADED: std::sync::OnceLock<Catalog> = std::sync::OnceLock::new();

pub fn get() -> &'static Catalog {
    LOADED.get_or_init(|| parse().expect("вшитый каталог не разобрался"))
}

pub fn find(id: &str) -> Option<&'static Tweak> {
    get().tweaks.iter().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn katalog_razbiraetsya() {
        let c = parse().expect("каталог должен разбираться");
        assert!(c.tweaks.len() > 40, "каталог подозрительно короткий");
        assert!(!c.presets.is_empty());
    }

    #[test]
    fn identifikatory_ne_povtoryayutsya() {
        // По id ищется настройка и по нему же хранится снимок для отката:
        // два одинаковых — и откат одной вернул бы значение другой.
        let c = parse().unwrap();
        let mut seen = HashSet::new();
        for t in &c.tweaks {
            assert!(seen.insert(t.id.clone()), "повтор id: {}", t.id);
        }
    }

    #[test]
    fn u_kazhdoj_nastrojki_est_obyasnenie() {
        for t in &parse().unwrap().tweaks {
            assert!(!t.title.trim().is_empty(), "{} без названия", t.id);
            assert!(
                t.detail.trim().chars().count() > 20,
                "{}: объяснение короче, чем нужно, чтобы понять последствия",
                t.id
            );
        }
    }

    #[test]
    fn hklm_trebuet_administratora() {
        // Запись в HKLM без прав молча не проходит: `reg add` отвечает
        // отказом, программа показывает ошибку, и человек не понимает, при
        // чём тут права. Флаг должен стоять в каталоге, а не выясняться на
        // живой машине.
        for t in &parse().unwrap().tweaks {
            let paths = reg_paths(&t.action);
            let hklm = paths
                .iter()
                .any(|p| p.to_uppercase().starts_with("HKLM") || p.to_uppercase().starts_with("HKEY_LOCAL_MACHINE"));
            if hklm {
                assert!(t.needs_admin, "{}: пишет в HKLM, но не просит прав", t.id);
            }
        }
    }

    #[test]
    fn sluzhby_zadaniya_i_pitanie_trebuyut_administratora() {
        for t in &parse().unwrap().tweaks {
            let needs = matches!(
                t.action,
                Action::Service { .. }
                    | Action::ScheduledTask { .. }
                    | Action::PowerScheme { .. }
                    | Action::PowerSetting { .. }
            );
            if needs {
                assert!(t.needs_admin, "{}: без прав это не применится", t.id);
            }
        }
    }

    #[test]
    fn udalenie_prilozhenij_otmecheno_neobratimym() {
        // Единственное необратимое действие в программе. Если отметка
        // потеряется, интерфейс нарисует его обычным переключателем.
        for t in &parse().unwrap().tweaks {
            if matches!(t.action, Action::AppxRemove { .. }) {
                assert_eq!(
                    t.risk,
                    Risk::Irreversible,
                    "{}: удаление пакета обязано быть помечено необратимым",
                    t.id
                );
            }
        }
    }

    #[test]
    fn u_komandy_est_obratnaya() {
        // Команда без обратной — это действие без отката, то есть то, чего
        // в каталоге быть не должно.
        for t in &parse().unwrap().tweaks {
            if let Action::Command { apply, revert, .. } = &t.action {
                assert!(!apply.is_empty(), "{}: пустая команда", t.id);
                assert!(!revert.is_empty(), "{}: команда без обратной", t.id);
            }
        }
    }

    #[test]
    fn nabory_ssylayutsya_na_sushchestvuyushchie_nastrojki() {
        let c = parse().unwrap();
        let ids: HashSet<&str> = c.tweaks.iter().map(|t| t.id.as_str()).collect();
        for p in &c.presets {
            assert!(!p.tweaks.is_empty(), "набор {} пустой", p.id);
            for id in &p.tweaks {
                assert!(ids.contains(id.as_str()), "набор {}: нет настройки {id}", p.id);
            }
        }
    }

    #[test]
    fn nabory_ne_tyanut_neobratimoe() {
        // «Применить набор» человек нажимает одной кнопкой и не читает
        // каждый пункт. Необратимому там не место — его выбирают поимённо.
        let c = parse().unwrap();
        for p in &c.presets {
            for id in &p.tweaks {
                let t = c.tweaks.iter().find(|t| &t.id == id).unwrap();
                assert_ne!(
                    t.risk,
                    Risk::Irreversible,
                    "набор {} тянет необратимое {id}",
                    p.id
                );
            }
        }
    }

    #[test]
    fn vzaimoisklyuchayushchie_gruppy_ne_odinochki() {
        // Группа из одного — это галочка, прикинувшаяся выбором.
        let c = parse().unwrap();
        let mut counts: std::collections::HashMap<&str, usize> = Default::default();
        for t in &c.tweaks {
            if let Some(g) = &t.exclusive {
                *counts.entry(g.as_str()).or_default() += 1;
            }
        }
        for (group, n) in counts {
            assert!(n > 1, "группа {group} из одной настройки");
        }
    }

    fn reg_paths(action: &Action) -> Vec<String> {
        match action {
            Action::Reg { path, .. }
            | Action::RegGroup { path, .. }
            | Action::RegKeyBlank { path }
            | Action::RegRemove { path, .. } => vec![path.clone()],
            Action::RegEach { base, .. } => vec![base.clone()],
            // Проба — это чтение; права нужны самой команде, а не ей.
            _ => vec![],
        }
    }
}
