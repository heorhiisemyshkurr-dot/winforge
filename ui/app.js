// Интерфейс WinForge. Без сборщика и без зависимостей: одна страница, один
// файл, разметка собирается функцией h() через createElement.
//
// Через createElement, а не строками: половина того, что здесь показывается,
// пришла из реестра и из списка пакетов — пути, имена программ, командные
// строки. Склей это в innerHTML, и чужая строка в автозагрузке станет кодом
// внутри окна программы, которая ходит в реестр с правами администратора.

const invoke = window.__TAURI__.core.invoke;

const state = {
  catalog: [],
  presets: [],
  states: {},
  applied: [],
  admin: false,
  page: "overview",
  hardware: null,
  // Что нужно сделать, чтобы изменения вступили в силу. Собирается по мере
  // нажатий: человек должен узнать про перезапуск Проводника от программы,
  // а не из того, что ничего не поменялось.
  pending: new Set(),
};

const EFFECT = {
  immediate: "действует сразу",
  explorer_restart: "нужен перезапуск Проводника",
  sign_out: "после повторного входа",
  reboot: "после перезагрузки",
};

const RISK = { caution: "осторожно", irreversible: "необратимо" };

const PAGES = {
  overview: { title: "Обзор", lead: "Что за машина, что уже применено и наборы на один клик." },
  appearance: {
    title: "Внешний вид",
    lead: "Панель задач, Проводник, тема, курсор и обои. Всё обратимо: программа запоминает прежнее значение каждой настройки.",
  },
  privacy: {
    title: "Приватность",
    lead: "Телеметрия, реклама, подсказки, Copilot и Recall. Внизу — удаление встроенных приложений: единственное, что нельзя отменить.",
  },
  startup: {
    title: "Автозагрузка",
    lead: "Что запускается при входе в систему, и службы, которые можно не держать включёнными. Отключение записи автозагрузки делается той же отметкой, что ставит диспетчер задач.",
  },
  apps: {
    title: "Приложения",
    lead: "Установленные приложения Microsoft Store. Удаление необратимо: вернуть можно только установкой из магазина заново.",
  },
  performance: {
    title: "Производительность",
    lead: "Питание, процессор, видеокарта, диск и сеть. У каждого пункта написано, чем за него платят.",
  },
  applied: {
    title: "Применённое",
    lead: "Всё, что программа изменила, вместе с прежним значением. Отсюда любое изменение снимается по одному или всё сразу.",
  },
};

// ——— мелкие помощники ———

function h(tag, props, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props || {})) {
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (value !== null && value !== undefined) node.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return node;
}

let toastTimer = null;
function toast(message, bad) {
  const box = document.getElementById("toast");
  box.textContent = message;
  box.className = bad ? "toast bad" : "toast";
  box.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (box.hidden = true), bad ? 9000 : 4000);
}

function tag(text, kind) {
  return h("span", { class: kind ? `tag ${kind}` : "tag", text });
}

function appliedIds() {
  return new Set(state.applied.map((a) => a.id));
}

// ——— сообщения сверху ———

function renderBanners() {
  const box = document.getElementById("banners");
  box.replaceChildren();

  if (!state.admin) {
    box.append(
      h(
        "div",
        { class: "banner warn" },
        h(
          "p",
          {},
          "Программа запущена без прав администратора. Настройки с пометкой «права» применить нельзя: " +
            "службы, задания, питание и ветки HKLM без них не меняются."
        ),
        h("button", { class: "btn", onclick: elevate }, "Перезапустить с правами")
      )
    );
  }

  if (state.pending.has("explorer_restart")) {
    box.append(
      h(
        "div",
        { class: "banner" },
        h(
          "p",
          {},
          "Часть изменений внешнего вида Проводник читает при запуске. Пока он не перезапущен, на экране всё по-старому."
        ),
        h(
          "button",
          {
            class: "btn",
            onclick: async (event) => {
              event.target.disabled = true;
              try {
                await invoke("restart_explorer");
                state.pending.delete("explorer_restart");
                toast("Проводник перезапущен.");
              } catch (e) {
                toast(String(e), true);
              }
              renderBanners();
            },
          },
          "Перезапустить Проводник"
        )
      )
    );
  }

  for (const [key, text] of [
    ["sign_out", "Часть изменений вступит в силу после выхода и повторного входа в систему."],
    ["reboot", "Часть изменений вступит в силу после перезагрузки компьютера."],
  ]) {
    if (!state.pending.has(key)) continue;
    box.append(h("div", { class: "banner" }, h("p", {}, text)));
  }
}

async function elevate() {
  try {
    await invoke("elevate");
  } catch (e) {
    toast(String(e), true);
  }
}

// ——— карточка настройки ———

function tweakCard(tweak) {
  const current = state.states[tweak.id] || "unknown";
  const ours = appliedIds().has(tweak.id);
  const on = current === "on";
  // «Чужое» — настройка уже стоит, но не нашими руками. Снять её нечем:
  // прежнего значения программа не знает, а подставлять своё представление
  // о «значении по умолчанию» — это не откат, а ещё одно изменение.
  const foreign = on && !ours;

  const switchNode = h("button", {
    class: "switch",
    "aria-pressed": String(on),
    "data-state": current,
    title: foreign ? "Уже стоит в системе — не программой" : "",
    onclick: (event) => toggleTweak(tweak, { on, ours, foreign }, event.currentTarget),
  });

  const badges = [
    tweak.needs_admin ? tag("права", "admin") : null,
    RISK[tweak.risk] ? tag(RISK[tweak.risk], tweak.risk) : null,
    tweak.effect !== "immediate" ? tag(EFFECT[tweak.effect]) : null,
    foreign ? tag("уже стоит") : null,
  ];

  const card = h(
    "div",
    { class: on ? "item is-on" : "item" },
    h("div", { class: "item-head" }, tweak.title, ...badges.filter(Boolean)),
    h("p", {}, tweak.detail, " ", h("button", { class: "link", onclick: showAction }, "что именно меняется")),
    h("div", { class: "side-cell" }, switchNode)
  );

  function showAction(event) {
    const old = card.querySelector(".detail");
    if (old) {
      old.remove();
      event.target.textContent = "что именно меняется";
      return;
    }
    event.target.textContent = "скрыть";
    card.append(h("div", { class: "detail", text: JSON.stringify(tweak.action, null, 2) }));
  }

  return card;
}

async function toggleTweak(tweak, status, node) {
  if (status.foreign) {
    toast(
      "Эта настройка уже стоит в системе, и поставила её не программа. Снять её нечем: " +
        "прежнего значения программа не знает. " +
        (tweak.exclusive ? "Выберите другой пункт этой же группы." : ""),
      true
    );
    return;
  }

  node.classList.add("busy");
  try {
    if (status.ours) {
      await invoke("revert", { id: tweak.id });
      toast(`Снято: ${tweak.title}`);
    } else {
      await invoke("apply", { id: tweak.id });
      toast(`Применено: ${tweak.title}`);
      if (tweak.effect !== "immediate") state.pending.add(tweak.effect);
    }
    state.applied = await invoke("applied");
    // Перечитываем состояние всей взаимоисключающей группы: включив одну
    // схему питания, мы выключили остальные, и их переключатели обязаны это
    // показать сами, без кнопки «обновить».
    const group = tweak.exclusive
      ? state.catalog.filter((t) => t.exclusive === tweak.exclusive)
      : [tweak];
    for (const member of group) {
      state.states[member.id] = await invoke("state", { id: member.id });
    }
  } catch (e) {
    toast(String(e), true);
  }
  node.classList.remove("busy");
  renderAll();
}

// ——— страницы ———

function pageCategory(category) {
  const tweaks = state.catalog.filter((t) => t.category === category);
  const reversible = tweaks.filter((t) => t.risk !== "irreversible");
  const irreversible = tweaks.filter((t) => t.risk === "irreversible");

  const parts = [h("div", { class: "list" }, reversible.map(tweakCard))];
  if (irreversible.length) {
    parts.push(
      h("h2", {}, "Удаление приложений — отменить нельзя"),
      h(
        "p",
        { class: "muted" },
        "Эти пункты удаляют пакет из системы. Вернуть его программа не может: только установка из Microsoft Store заново."
      ),
      h("div", { class: "list" }, irreversible.map(tweakCard))
    );
  }
  return parts;
}

function pageOverview() {
  const hw = state.hardware;
  const tile = (label, value) => h("div", { class: "tile" }, h("b", { text: label }), h("span", { text: value }));

  const tiles = hw
    ? h(
        "div",
        { class: "tiles" },
        tile("Система", `${hw.os || "Windows"} (сборка ${hw.build || "?"})`),
        tile("Процессор", `${hw.cpu || "?"} — ${hw.cores || "?"} ядер / ${hw.threads || "?"} потоков`),
        tile("Видеокарта", (hw.gpu || []).join(", ") || "?"),
        tile(
          "Память",
          `${hw.ram_gb || "?"} ГБ${hw.ram_speed ? `, ${hw.ram_speed} МГц` : ""}${
            hw.ram_sticks ? `, планок: ${hw.ram_sticks}` : ""
          } · свободно ${hw.ram_free_gb || "?"} ГБ`
        ),
        tile("Материнская плата", hw.board || "?"),
        tile("Диски", (hw.disks || []).map((d) => `${d.model} (${d.size_gb} ГБ)`).join(" · ") || "?"),
        tile("Без перезагрузки", `${hw.uptime_hours || "?"} ч`)
      )
    : h("div", { class: "spin", text: "читаю опись машины…" });

  const presets = h(
    "div",
    { class: "list" },
    state.presets.map((preset) =>
      h(
        "div",
        { class: "item" },
        h("div", { class: "item-head" }, preset.title, tag(`${preset.tweaks.length} настроек`)),
        h("p", {}, preset.detail),
        h(
          "div",
          { class: "side-cell" },
          h(
            "button",
            {
              class: "btn",
              onclick: (event) => applyPreset(preset, event.currentTarget),
            },
            "Применить"
          )
        )
      )
    )
  );

  return [
    h("h2", {}, "Эта машина"),
    tiles,
    h("h2", {}, "Перед большими изменениями"),
    h(
      "div",
      { class: "banner" },
      h(
        "p",
        {},
        "Точка восстановления — страховка на случай, когда виновата не программа, а что-то ещё. " +
          "Windows делает не больше одной точки в сутки и только при включённой защите системы; " +
          "программа скажет прямо, что из этого получилось."
      ),
      h(
        "button",
        {
          class: "ghost",
          onclick: async (event) => {
            event.target.disabled = true;
            event.target.textContent = "создаю…";
            try {
              const result = await invoke("restore_point");
              const said = {
                created: "Точка восстановления создана.",
                skipped:
                  "Windows не стала делать новую точку: за последние сутки она уже есть. Это её ограничение, не наше.",
                "no-protection":
                  "Защита системы выключена на системном диске — точку создать нельзя. Включается в «Свойства системы» → «Защита системы».",
              };
              toast(said[result] || result, result !== "created");
            } catch (e) {
              toast(String(e), true);
            }
            event.target.disabled = false;
            event.target.textContent = "Создать точку восстановления";
          },
        },
        "Создать точку восстановления"
      )
    ),
    h("h2", {}, "Наборы"),
    presets,
  ];
}

async function applyPreset(preset, button) {
  button.disabled = true;
  button.textContent = "применяю…";
  try {
    const report = await invoke("apply_preset", { id: preset.id });
    state.applied = await invoke("applied");
    for (const id of report.done) {
      const tweak = state.catalog.find((t) => t.id === id);
      if (tweak && tweak.effect !== "immediate") state.pending.add(tweak.effect);
    }
    state.states = await invoke("states");
    if (report.failed.length === 0) {
      toast(`Набор «${preset.title}» применён: ${report.done.length} настроек.`);
    } else {
      toast(
        `Применено ${report.done.length}, не удалось ${report.failed.length}: ` +
          report.failed.map((f) => `${f.title} — ${f.error}`).join("; "),
        true
      );
    }
  } catch (e) {
    toast(String(e), true);
  }
  button.disabled = false;
  button.textContent = "Применить";
  renderAll();
}

function pageApplied() {
  if (state.applied.length === 0) {
    return [h("p", { class: "muted" }, "Программа пока ничего не меняла.")];
  }
  const rows = state.applied
    .slice()
    .reverse()
    .map((entry) =>
      h(
        "div",
        { class: "item" },
        h(
          "div",
          { class: "item-head" },
          entry.title,
          tag(new Date(entry.at * 1000).toLocaleString("ru-RU"))
        ),
        h("p", { class: "muted" }, "Прежнее значение: ", JSON.stringify(entry.snapshot)),
        h(
          "div",
          { class: "side-cell" },
          h(
            "button",
            {
              class: "ghost",
              onclick: async (event) => {
                event.target.disabled = true;
                try {
                  await invoke("revert", { id: entry.id });
                  state.applied = await invoke("applied");
                  state.states[entry.id] = await invoke("state", { id: entry.id });
                  toast(`Снято: ${entry.title}`);
                } catch (e) {
                  toast(String(e), true);
                  event.target.disabled = false;
                }
                renderAll();
              },
            },
            "Вернуть как было"
          )
        )
      )
    );

  return [
    h(
      "div",
      { class: "row" },
      h(
        "button",
        {
          class: "danger",
          onclick: async (event) => {
            event.target.disabled = true;
            event.target.textContent = "возвращаю…";
            const failed = await invoke("revert_all");
            state.applied = await invoke("applied");
            state.states = await invoke("states");
            toast(
              failed.length
                ? `Вернулось не всё, осталось: ${failed.join("; ")}`
                : "Все изменения программы сняты.",
              failed.length > 0
            );
            renderAll();
          },
        },
        "Вернуть всё как было"
      ),
      h("span", { class: "muted" }, `${state.applied.length} изменений`)
    ),
    h("div", { class: "list" }, rows),
  ];
}

function pageStartup() {
  const services = state.catalog.filter((t) => t.category === "startup");
  const box = h("div", { class: "spin", text: "читаю автозагрузку…" });

  invoke("startup_items")
    .then((items) => {
      if (items.length === 0) {
        box.replaceWith(h("p", { class: "muted" }, "В автозагрузке ничего нет."));
        return;
      }
      box.replaceWith(
        h(
          "div",
          { class: "list" },
          items.map((item) =>
            h(
              "div",
              { class: item.enabled ? "item is-on" : "item" },
              h(
                "div",
                { class: "item-head" },
                item.name,
                tag(item.source),
                item.needs_admin ? tag("права", "admin") : null
              ),
              h("p", { class: "muted" }, item.command),
              h(
                "div",
                { class: "side-cell" },
                h("button", {
                  class: "switch",
                  "aria-pressed": String(item.enabled),
                  onclick: async (event) => {
                    const node = event.currentTarget;
                    node.classList.add("busy");
                    try {
                      await invoke("startup_toggle", { id: item.id, enabled: !item.enabled });
                      item.enabled = !item.enabled;
                      node.setAttribute("aria-pressed", String(item.enabled));
                      node.closest(".item").className = item.enabled ? "item is-on" : "item";
                      toast(
                        item.enabled
                          ? `${item.name} будет запускаться при входе.`
                          : `${item.name} больше не запускается при входе.`
                      );
                    } catch (e) {
                      toast(String(e), true);
                    }
                    node.classList.remove("busy");
                  },
                })
              )
            )
          )
        )
      );
    })
    .catch((e) => box.replaceWith(h("p", { class: "muted" }, String(e))));

  return [
    h("h2", {}, "Запускается при входе"),
    box,
    h("h2", {}, "Службы и задержка запуска"),
    h("div", { class: "list" }, services.map(tweakCard)),
  ];
}

function pageApps() {
  const box = h("div", { class: "spin", text: "читаю список приложений…" });

  invoke("apps")
    .then((apps) => {
      box.replaceWith(
        h(
          "div",
          { class: "list" },
          apps.map((app) => {
            const card = h(
              "div",
              { class: "item" },
              h("div", { class: "item-head" }, app.name, tag(app.publisher)),
              h("p", { class: "muted" }, app.full_name)
            );
            // Два нажатия вместо окна подтверждения: удаление необратимо, и
            // случайный щелчок не должен его запускать.
            let armed = false;
            const button = h(
              "button",
              {
                class: "danger",
                onclick: async () => {
                  if (!armed) {
                    armed = true;
                    button.textContent = "Точно удалить?";
                    setTimeout(() => {
                      armed = false;
                      button.textContent = "Удалить";
                    }, 4000);
                    return;
                  }
                  button.disabled = true;
                  button.textContent = "удаляю…";
                  try {
                    await invoke("remove_app", { fullName: app.full_name });
                    card.remove();
                    toast(`${app.name} удалён. Вернуть можно только из Microsoft Store.`);
                  } catch (e) {
                    toast(String(e), true);
                    button.disabled = false;
                    button.textContent = "Удалить";
                  }
                },
              },
              "Удалить"
            );
            card.append(h("div", { class: "side-cell" }, button));
            return card;
          })
        )
      );
    })
    .catch((e) => box.replaceWith(h("p", { class: "muted" }, String(e))));

  return [box];
}

function pageAppearance() {
  const input = h("input", { type: "text", placeholder: "C:\\Users\\...\\обои.jpg" });
  const style = h(
    "select",
    {},
    h("option", { value: "fill" }, "Заполнение"),
    h("option", { value: "fit" }, "По размеру"),
    h("option", { value: "stretch" }, "Растянуть"),
    h("option", { value: "center" }, "По центру"),
    h("option", { value: "tile" }, "Замостить")
  );
  const now = h("p", { class: "muted", text: "смотрю, какие обои стоят…" });

  invoke("wallpaper").then((path) => {
    now.textContent = path ? `Сейчас: ${path}` : "Сейчас обои не заданы.";
    input.value = path || "";
  });

  const wallpaper = h(
    "div",
    { class: "item" },
    h("div", { class: "item-head" }, "Обои рабочего стола"),
    h(
      "p",
      {},
      "Путь к файлу вписывается вручную: выбор файла окном потребовал бы отдельного разрешения у системы. " +
        "Прежний путь показан ниже — программа его не запоминает, так что скопируйте его, если захотите вернуть."
    ),
    now,
    h(
      "div",
      { class: "field" },
      input,
      style,
      h(
        "button",
        {
          class: "btn",
          onclick: async (event) => {
            event.target.disabled = true;
            try {
              await invoke("set_wallpaper", { path: input.value.trim(), style: style.value });
              toast("Обои поставлены.");
              now.textContent = `Сейчас: ${input.value.trim()}`;
            } catch (e) {
              toast(String(e), true);
            }
            event.target.disabled = false;
          },
        },
        "Поставить"
      )
    )
  );

  return [...pageCategory("appearance"), h("h2", {}, "Обои"), wallpaper];
}

// ——— сборка ———

function renderPage() {
  const info = PAGES[state.page];
  const page = document.getElementById("page");
  page.replaceChildren(h("h1", { text: info.title }), h("p", { class: "lead", text: info.lead }));

  const body =
    state.page === "overview"
      ? pageOverview()
      : state.page === "applied"
      ? pageApplied()
      : state.page === "startup"
      ? pageStartup()
      : state.page === "apps"
      ? pageApps()
      : state.page === "appearance"
      ? pageAppearance()
      : pageCategory(state.page);

  page.append(...body);
}

function renderAll() {
  document.getElementById("applied-count").textContent = String(state.applied.length);
  document.getElementById("admin-state").textContent = state.admin
    ? "права администратора есть"
    : "без прав администратора";
  document.getElementById("admin-state").className = state.admin ? "admin is-admin" : "admin";
  renderBanners();
  renderPage();
}

document.getElementById("nav").addEventListener("click", (event) => {
  const button = event.target.closest(".nav-item");
  if (!button) return;
  state.page = button.dataset.page;
  for (const item of document.querySelectorAll(".nav-item")) {
    item.classList.toggle("is-active", item === button);
  }
  renderPage();
});

document.getElementById("refresh").addEventListener("click", async (event) => {
  event.target.disabled = true;
  event.target.textContent = "читаю…";
  state.states = await invoke("states");
  state.applied = await invoke("applied");
  state.admin = await invoke("is_admin");
  event.target.disabled = false;
  event.target.textContent = "Перечитать состояние";
  renderAll();
});

async function start() {
  state.admin = await invoke("is_admin");
  state.catalog = await invoke("catalog");
  state.presets = await invoke("presets");
  state.applied = await invoke("applied");
  renderAll();

  // Опись машины и опрос состояния идут после первой отрисовки: оба
  // занимают секунды, и окно не должно ждать их, чтобы показать себя.
  invoke("hardware")
    .then((hw) => {
      state.hardware = hw;
      if (state.page === "overview") renderPage();
    })
    .catch(() => {});

  state.states = await invoke("states");
  renderAll();
}

start().catch((e) => toast(String(e), true));
