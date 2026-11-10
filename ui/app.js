"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const IS_MAC = /Mac|iPhone|iPad/.test(navigator.platform);

// Английские названия передаются в промпт модели, русские — для интерфейса.
const LANGS = [
  ["Russian", "Русский"], ["English", "English"], ["Ukrainian", "Українська"],
  ["German", "Deutsch"], ["French", "Français"], ["Spanish", "Español"],
  ["Italian", "Italiano"], ["Portuguese", "Português"], ["Polish", "Polski"],
  ["Turkish", "Türkçe"], ["Chinese", "中文"], ["Japanese", "日本語"],
  ["Korean", "한국어"], ["Arabic", "العربية"],
];

let state = null;          // ответ get_state
let busy = null;           // { mode: 'summarize' | 'translate', fromHotkey: bool } | null
let saveTimer = null;
let modelStatePoll = null;

// ---------- утилиты ----------
function fmtBytes(n) {
  if (!n) return "—";
  const u = ["Б", "КБ", "МБ", "ГБ"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return n.toFixed(i >= 2 ? 1 : 0) + " " + u[i];
}
function fmtSecs(ms) { return (ms / 1000).toFixed(1) + " с"; }
function toast(msg, ms = 3200) {
  const t = $("toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => (t.hidden = true), ms);
}
function fillLangSelect(sel, withAuto, autoLabel) {
  sel.innerHTML = "";
  if (withAuto) sel.append(new Option(autoLabel, "auto"));
  for (const [v, l] of LANGS) sel.append(new Option(l, v));
}
const countText = (el, out) => (out.textContent = el.value.length.toLocaleString("ru") + " символов");

// ---------- вкладки ----------
function showTab(name) {
  document.querySelectorAll(".tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  document.querySelectorAll(".panel").forEach((p) => p.classList.toggle("active", p.id === "tab-" + name));
}
document.querySelectorAll(".tab").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.tab)));

// ---------- состояние ----------
async function refreshState() {
  state = await invoke("get_state");
  renderState();
}

function renderState() {
  const s = state.settings;
  $("chip-backend").textContent = state.backend + (state.backend === "CPU" ? "" : " · GPU");

  // Мастер первого запуска
  const needSetup = !state.model_ready && !state.downloading && !downloadActive;
  $("setup").hidden = !(needSetup || setupForced);
  if (!$("setup").hidden) renderSetup();

  // Настройки
  $("set-ctx").value = s.n_ctx; $("ctx-value").textContent = s.n_ctx;
  $("set-out").value = s.max_output_tokens; $("out-value").textContent = s.max_output_tokens;
  $("set-gpu").value = s.gpu_mode;
  $("set-threads").value = s.threads;
  $("set-fast").checked = s.fast_restart;
  $("set-keep").value = s.keep_alive_minutes; $("keep-value").textContent = s.keep_alive_minutes;
  $("keep-wrap").style.opacity = s.fast_restart ? 1 : 0.45;
  $("set-autostart").checked = s.autostart;
  $("set-autocopy").checked = s.auto_copy_result;
  $("hk-summarize").value = prettyHotkey(s.hotkey_summarize);
  $("hk-translate").value = prettyHotkey(s.hotkey_translate);
  $("hk-open").value = prettyHotkey(s.hotkey_open);
  $("hk-summarize").dataset.value = s.hotkey_summarize;
  $("hk-translate").dataset.value = s.hotkey_translate;
  $("hk-open").dataset.value = s.hotkey_open;
  $("set-sumlen").value = s.summary_length;
  $("set-sumlang").value = s.summary_language;
  $("set-trtarget").value = s.translate_target;
  $("set-trsecondary").value = s.translate_secondary;
  $("models-dir").textContent = state.models_dir;
  renderModelList();
  setModelChip(state.model_loaded, null);
}

function setModelChip(loaded, device) {
  const chip = $("chip-model");
  chip.textContent = loaded ? "Модель в памяти" + (device ? " · " + device : "") : "Модель выгружена";
  chip.classList.toggle("on", loaded);
  $("btn-unload").hidden = !loaded || !!busy;
}

function collectSettings() {
  const s = { ...state.settings };
  s.n_ctx = +$("set-ctx").value;
  s.max_output_tokens = +$("set-out").value;
  s.gpu_mode = $("set-gpu").value;
  s.threads = Math.max(0, +$("set-threads").value || 0);
  s.fast_restart = $("set-fast").checked;
  s.keep_alive_minutes = +$("set-keep").value;
  s.autostart = $("set-autostart").checked;
  s.auto_copy_result = $("set-autocopy").checked;
  s.hotkey_summarize = $("hk-summarize").dataset.value || "";
  s.hotkey_translate = $("hk-translate").dataset.value || "";
  s.hotkey_open = $("hk-open").dataset.value || "";
  s.summary_length = $("set-sumlen").value;
  s.summary_language = $("set-sumlang").value;
  s.translate_target = $("set-trtarget").value;
  s.translate_secondary = $("set-trsecondary").value;
  return s;
}

function scheduleSave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(async () => {
    try {
      const warnings = await invoke("save_settings", { newSettings: collectSettings() });
      const w = $("hk-warn");
      w.hidden = warnings.length === 0;
      w.textContent = warnings.length ? "Не удалось зарегистрировать:\n" + warnings.join("\n") : "";
      await refreshState();
    } catch (e) { toast("Ошибка сохранения: " + e); }
  }, 350);
}

["set-ctx", "set-out", "set-keep"].forEach((id) =>
  $(id).addEventListener("input", () => {
    $("ctx-value").textContent = $("set-ctx").value;
    $("out-value").textContent = $("set-out").value;
    $("keep-value").textContent = $("set-keep").value;
    scheduleSave();
  }));
["set-gpu", "set-threads", "set-fast", "set-autostart", "set-autocopy",
 "set-sumlen", "set-sumlang", "set-trtarget", "set-trsecondary"].forEach((id) =>
  $(id).addEventListener("change", scheduleSave));

// ---------- горячие клавиши (запись сочетания) ----------
function prettyHotkey(s) {
  if (!s) return "";
  return s.replace("CommandOrControl", IS_MAC ? "⌘" : "Ctrl").replace("Super", IS_MAC ? "⌘" : "Win")
    .replace("Alt", IS_MAC ? "⌥" : "Alt").replace("Control", "Ctrl");
}
function keyName(e) {
  const c = e.code;
  if (/^Key[A-Z]$/.test(c)) return c.slice(3);
  if (/^Digit\d$/.test(c)) return c.slice(5);
  if (/^F\d{1,2}$/.test(c)) return c;
  const map = { Space: "Space", Enter: "Enter", Tab: "Tab", ArrowUp: "Up", ArrowDown: "Down",
    ArrowLeft: "Left", ArrowRight: "Right", Minus: "-", Equal: "=", Comma: ",", Period: ".",
    Slash: "/", Backslash: "\\", Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]",
    Backquote: "`", Home: "Home", End: "End", PageUp: "PageUp", PageDown: "PageDown", Insert: "Insert", Delete: "Delete" };
  return map[c] || null;
}
document.querySelectorAll("input.hotkey").forEach((inp) => {
  inp.addEventListener("focus", () => { inp.classList.add("recording"); inp.placeholder = "Нажмите сочетание…"; });
  inp.addEventListener("blur", () => { inp.classList.remove("recording"); inp.placeholder = ""; });
  inp.addEventListener("keydown", (e) => {
    e.preventDefault();
    if (e.key === "Backspace" || e.key === "Delete") {
      inp.dataset.value = ""; inp.value = ""; scheduleSave(); return;
    }
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
    const key = keyName(e);
    if (!key) return;
    const mods = [];
    if (IS_MAC) { if (e.metaKey) mods.push("CommandOrControl"); if (e.ctrlKey) mods.push("Control"); }
    else { if (e.ctrlKey) mods.push("CommandOrControl"); if (e.metaKey) mods.push("Super"); }
    if (e.altKey) mods.push("Alt");
    if (e.shiftKey) mods.push("Shift");
    if (!mods.length) { toast("Добавьте модификатор (Ctrl/Alt/Shift/Cmd)"); return; }
    const value = [...mods, key].join("+");
    inp.dataset.value = value; inp.value = prettyHotkey(value);
    inp.blur();
    scheduleSave();
  });
});

// ---------- модели / загрузка ----------
let downloadActive = false;
let setupForced = false;

function renderModelList() {
  const box = $("model-list");
  box.innerHTML = "";
  for (const p of state.presets) {
    const item = document.createElement("div");
    const presetActive = !state.settings.use_custom_model && state.settings.model_preset === p.id;
    item.className = "model-item" + (presetActive ? " active" : "");
    item.innerHTML = `<div class="title"></div><div class="hint"></div>`;
    item.querySelector(".title").textContent = p.title;
    item.querySelector(".hint").textContent = p.note + " · " + p.repo;
    for (const q of p.quants) {
      const row = document.createElement("div");
      row.className = "quant-row";
      const isActive = presetActive && state.settings.quant === q.id;
      row.innerHTML = `<span class="label"></span><span class="badge"></span>`;
      row.querySelector(".label").textContent = `${q.label}${q.size ? " · " + fmtBytes(q.size) : ""}`;
      const badge = row.querySelector(".badge");
      badge.textContent = q.installed ? (isActive ? "используется" : "скачана") : "не скачана";
      badge.classList.toggle("ok", q.installed);
      if (q.installed) {
        if (!isActive) {
          const use = mkBtn("Использовать", async () => {
            const s = collectSettings(); s.model_preset = p.id; s.quant = q.id; s.use_custom_model = false;
            await invoke("save_settings", { newSettings: s }); await refreshState();
          });
          row.append(use);
        }
        row.append(mkBtn("Удалить", async () => {
          if (!confirm(`Удалить файл ${q.file}?`)) return;
          try { await invoke("delete_model", { preset: p.id, quant: q.id }); } catch (e) { toast(e); }
          await refreshState();
        }));
      } else {
        row.append(mkBtn("Скачать", () => { setupForced = true; selectedSetup = { preset: p.id, quant: q.id }; renderState(); }));
      }
      item.append(row);
    }
    box.append(item);
  }
  renderCustomModel();
}

function renderCustomModel() {
  const s = state.settings;
  const has = !!state.custom_model_name;
  const active = s.use_custom_model && has;
  $("custom-model").classList.toggle("active", active);
  $("custom-name").textContent = has
    ? `${state.custom_model_name} · ${fmtBytes(state.custom_model_size)}`
    : (s.custom_model_path ? "Файл не найден: " + s.custom_model_path : "Файл не выбран");
  $("custom-name").title = s.custom_model_path || "";
  const badge = $("custom-badge");
  badge.textContent = active ? "используется" : (has ? "выбран" : "");
  badge.hidden = !has;
  badge.classList.toggle("ok", has);
  $("custom-use").hidden = !has || active;
}

async function pickCustomModel() {
  try {
    const path = await invoke("pick_custom_model");
    if (path) {
      setupForced = false;
      toast("Модель выбрана: " + path.split(/[\\/]/).pop());
      await refreshState();
    }
  } catch (e) { toast(String(e), 6000); }
}
$("custom-pick").addEventListener("click", pickCustomModel);
$("setup-pick").addEventListener("click", pickCustomModel);
$("custom-use").addEventListener("click", async () => {
  const s = collectSettings(); s.use_custom_model = true;
  await invoke("save_settings", { newSettings: s }); await refreshState();
});

function mkBtn(text, fn) {
  const b = document.createElement("button");
  b.className = "btn small"; b.textContent = text; b.addEventListener("click", fn);
  return b;
}

let selectedSetup = { preset: "qwen38", quant: "q4km" };

function renderSetup() {
  const sel = $("setup-preset");
  if (!sel.options.length) {
    for (const p of state.presets) sel.append(new Option(p.title, p.id));
    sel.addEventListener("change", () => { selectedSetup.preset = sel.value; renderSetup(); });
  }
  sel.value = selectedSetup.preset;
  sel.disabled = downloadActive;
  const preset = state.presets.find((p) => p.id === selectedSetup.preset) || state.presets[0];
  $("setup-note").textContent = `${preset.note} Репозиторий: huggingface.co/${preset.repo}`;
  const box = $("setup-quants");
  box.innerHTML = "";
  const hints = { q4km: "Быстрее и легче — рекомендуется для большинства ПК", q8: "Максимальное качество, почти без потерь, нужно больше памяти" };
  for (const q of preset.quants) {
    const lab = document.createElement("label");
    lab.className = "quant-opt";
    lab.innerHTML = `<input type="radio" name="quant"><span class="info"><b></b><small></small></span><span class="badge"></span>`;
    const radio = lab.querySelector("input");
    radio.value = q.id; radio.checked = selectedSetup.quant === q.id; radio.disabled = downloadActive;
    radio.addEventListener("change", () => (selectedSetup.quant = q.id));
    lab.querySelector("b").textContent = q.label;
    lab.querySelector("small").textContent = hints[q.id];
    const badge = lab.querySelector(".badge");
    badge.textContent = q.installed ? "скачана" : (q.size ? fmtBytes(q.size) : "≈ 3–5 ГБ");
    badge.classList.toggle("ok", q.installed);
    box.append(lab);
  }
  const installed = preset.quants.find((q) => q.id === selectedSetup.quant)?.installed;
  $("setup-download").textContent = installed ? "Использовать" : "Скачать";
  $("setup-download").disabled = downloadActive;
  $("setup-pick").disabled = downloadActive;
  $("setup-cancel").hidden = !downloadActive;
}

$("setup-download").addEventListener("click", async () => {
  const preset = state.presets.find((p) => p.id === selectedSetup.preset);
  const q = preset.quants.find((x) => x.id === selectedSetup.quant);
  try {
    if (q.installed) {
      const s = collectSettings(); s.model_preset = preset.id; s.quant = q.id; s.use_custom_model = false;
      await invoke("save_settings", { newSettings: s });
      setupForced = false; await refreshState(); return;
    }
    downloadActive = true;
    $("setup-progress").hidden = false;
    $("setup-progress").classList.add("indeterminate");
    $("setup-status").textContent = "Подключение к HuggingFace…";
    renderSetup();
    await invoke("start_download", { preset: preset.id, quant: q.id });
  } catch (e) {
    downloadActive = false; $("setup-status").textContent = "Ошибка: " + e; renderSetup();
  }
});
$("setup-cancel").addEventListener("click", () => invoke("cancel_download"));

listen("download-event", async ({ payload: d }) => {
  const bar = $("setup-progress");
  if (d.kind === "progress") {
    downloadActive = true;
    bar.hidden = false; bar.classList.remove("indeterminate");
    const pct = d.total ? (d.downloaded / d.total) * 100 : 0;
    bar.firstElementChild.style.width = pct.toFixed(1) + "%";
    const eta = d.speed_bps > 0 && d.total ? Math.round((d.total - d.downloaded) / d.speed_bps) : 0;
    $("setup-status").textContent = `${fmtBytes(d.downloaded)} из ${fmtBytes(d.total)} (${pct.toFixed(1)}%) · ${fmtBytes(d.speed_bps)}/с` +
      (eta ? ` · осталось ~${Math.floor(eta / 60)} мин ${eta % 60} с` : "");
  } else if (d.kind === "done") {
    downloadActive = false; setupForced = false;
    bar.hidden = true; $("setup-status").textContent = "";
    toast("Модель скачана и готова к работе");
    await refreshState();
  } else if (d.kind === "cancelled") {
    downloadActive = false; bar.classList.remove("indeterminate");
    $("setup-status").textContent = d.message; await refreshState();
  } else if (d.kind === "error") {
    downloadActive = false; bar.classList.remove("indeterminate");
    $("setup-status").textContent = "Ошибка: " + d.message; await refreshState();
  }
  if (!$("setup").hidden) renderSetup();
});

$("btn-open-models").addEventListener("click", () => invoke("open_models_dir").catch(toast));

// ---------- задачи: сводка / перевод ----------
const UI = {
  summarize: { input: "sum-input", result: "sum-result", status: "sum-status", progress: "sum-progress",
    run: "sum-run", stop: "sum-stop", copy: "sum-copy" },
  translate: { input: "tr-input", result: "tr-result", status: "tr-status", progress: "tr-progress",
    run: "tr-run", stop: "tr-stop", copy: "tr-copy" },
};

function setBusy(mode, fromHotkey) {
  busy = mode ? { mode, fromHotkey: !!fromHotkey } : null;
  for (const m of Object.keys(UI)) {
    const u = UI[m];
    const mine = busy && busy.mode === m;
    $(u.run).hidden = !!mine; $(u.stop).hidden = !mine;
    $(u.run).disabled = !!busy;
  }
  $("btn-unload").hidden = !!busy || !state?.model_loaded;
}

async function startJob(mode, text, fromHotkey = false) {
  if (busy) { toast("Подождите завершения текущей задачи"); return; }
  if (!state.model_ready) { setupForced = false; renderState(); return; }
  const u = UI[mode];
  const result = $(u.result);
  result.textContent = ""; result.classList.remove("error");
  $(u.copy).disabled = true;
  $(u.status).textContent = "Запуск…";
  $(u.progress).hidden = false; $(u.progress).classList.add("indeterminate");
  $(u.progress).firstElementChild.style.width = "";
  if (mode === "summarize") { $("sum-partials").innerHTML = ""; $("sum-partials-wrap").hidden = true; $("sum-partials-count").textContent = "0"; }
  else $("tr-lang-note").textContent = "";
  $("st-speed").textContent = "Скорость: —"; $("st-detail").textContent = "";
  setBusy(mode, fromHotkey);
  const req = mode === "summarize"
    ? { mode, text, length: $("sum-length").value, language: $("sum-lang").value }
    : { mode, text, target: $("tr-target").value };
  try {
    await invoke("run_job", { req });
  } catch (e) {
    finishJob(); result.textContent = String(e); result.classList.add("error");
    $(u.status).textContent = "";
  }
}

function finishJob() {
  if (!busy) return;
  const u = UI[busy.mode];
  $(u.progress).hidden = true; $(u.progress).classList.remove("indeterminate");
  setBusy(null);
}

listen("job-event", async ({ payload: e }) => {
  if (!busy) return;
  const mode = busy.mode, u = UI[mode];
  switch (e.kind) {
    case "status": {
      $(u.status).textContent = e.message;
      const bar = $(u.progress);
      if (e.total > 1) {
        bar.classList.remove("indeterminate");
        const base = e.stage === "merge" ? 0.0 : 0;
        bar.firstElementChild.style.width = (((e.current - 1 + base) / e.total) * 100).toFixed(0) + "%";
      } else bar.classList.add("indeterminate");
      break;
    }
    case "token": {
      const r = $(u.result);
      r.textContent += e.text;
      r.scrollTop = r.scrollHeight;
      break;
    }
    case "partial": {
      const wrap = $("sum-partials-wrap");
      wrap.hidden = false;
      const div = document.createElement("div");
      div.className = "partial";
      const b = document.createElement("b"); b.textContent = e.label;
      div.append(b, document.createTextNode(e.text));
      $("sum-partials").append(div);
      $("sum-partials-count").textContent = $("sum-partials").children.length;
      break;
    }
    case "target": $("tr-lang-note").textContent = "→ " + (LANGS.find((l) => l[0] === e.language)?.[1] || e.language); break;
    case "stats": renderStats(e, false); break;
    case "done": {
      $(u.result).textContent = e.text;
      $(u.copy).disabled = !e.text;
      $(u.status).textContent = "Готово";
      renderStats(e, true);
      const auto = busy.fromHotkey && state.settings.auto_copy_result && e.text;
      finishJob();
      if (auto) invoke("copy_text", { text: e.text }).then(() => toast("Результат скопирован"));
      break;
    }
    case "cancelled": $(u.status).textContent = "Остановлено"; $(u.copy).disabled = !$(u.result).textContent; finishJob(); break;
    case "error": {
      const r = $(u.result);
      if (!r.textContent) { r.textContent = e.message; r.classList.add("error"); } else toast(e.message, 6000);
      $(u.status).textContent = "Ошибка"; finishJob(); break;
    }
  }
  if (e.kind === "done" || e.kind === "error" || e.kind === "cancelled") await refreshState().catch(() => {});
});

function renderStats(s, final) {
  const gen = s.gen_tps ? s.gen_tps.toFixed(1) : "—";
  $("st-speed").textContent = `Скорость: ${gen} ток/с`;
  const parts = [];
  if (s.prompt_tps) parts.push(`обработка промпта ${s.prompt_tps.toFixed(0)} ток/с`);
  parts.push(`${s.gen_tokens || 0} ток. сгенерировано`);
  parts.push(`${s.prompt_tokens || 0} ток. на входе`);
  parts.push(fmtSecs(s.elapsed_ms || 0));
  if (final && s.device) parts.push(s.device + ` · ctx ${s.n_ctx}`);
  $("st-detail").textContent = parts.join(" · ");
}

listen("model-state", ({ payload: m }) => {
  if (state) state.model_loaded = m.loaded;
  setModelChip(m.loaded, m.device);
});

// События горячих клавиш
listen("hotkey-action", ({ payload: h }) => {
  if (h.mode === "open") { showTab("summary"); $("sum-input").focus(); return; }
  if (h.error) toast(h.error, 6000);
  if (!h.text) { if (!h.error) toast("Выделенный текст не найден — вставьте его вручную"); showTab(h.mode === "translate" ? "translate" : "summary"); return; }
  if (h.mode === "translate") {
    showTab("translate"); $("tr-input").value = h.text; countText($("tr-input"), $("tr-count"));
    $("tr-target").value = "auto";
    startJob("translate", h.text, true);
  } else {
    showTab("summary"); $("sum-input").value = h.text; countText($("sum-input"), $("sum-count"));
    startJob("summarize", h.text, true);
  }
});

// Кнопки
$("sum-run").addEventListener("click", () => { const t = $("sum-input").value.trim(); t ? startJob("summarize", t) : toast("Вставьте текст"); });
$("tr-run").addEventListener("click", () => { const t = $("tr-input").value.trim(); t ? startJob("translate", t) : toast("Введите текст"); });
$("sum-stop").addEventListener("click", () => invoke("cancel_job"));
$("tr-stop").addEventListener("click", () => invoke("cancel_job"));
$("sum-clear").addEventListener("click", () => { $("sum-input").value = ""; $("sum-result").textContent = ""; $("sum-copy").disabled = true; countText($("sum-input"), $("sum-count")); $("sum-input").focus(); });
$("tr-clear").addEventListener("click", () => { $("tr-input").value = ""; $("tr-result").textContent = ""; $("tr-copy").disabled = true; countText($("tr-input"), $("tr-count")); $("tr-input").focus(); });
for (const [btn, res] of [["sum-copy", "sum-result"], ["tr-copy", "tr-result"]]) {
  $(btn).addEventListener("click", async () => {
    try { await invoke("copy_text", { text: $(res).textContent }); toast("Скопировано"); } catch (e) { toast("Не удалось скопировать: " + e); }
  });
}
$("sum-input").addEventListener("input", () => countText($("sum-input"), $("sum-count")));
$("tr-input").addEventListener("input", () => countText($("tr-input"), $("tr-count")));
$("sum-input").addEventListener("keydown", (e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter") $("sum-run").click(); });
$("tr-input").addEventListener("keydown", (e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter") $("tr-run").click(); });
$("btn-hide").addEventListener("click", () => invoke("hide_window"));
$("btn-unload").addEventListener("click", async () => {
  const ok = await invoke("unload_model");
  toast(ok ? "Модель выгружена из памяти" : "Модель занята генерацией");
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && $("setup").hidden && !document.activeElement?.classList?.contains("hotkey")) invoke("hide_window");
});

// ---------- старт ----------
(async function init() {
  fillLangSelect($("sum-lang"), true, "Как в оригинале");
  fillLangSelect($("set-sumlang"), true, "Как в оригинале");
  fillLangSelect($("tr-target"), true, "Авто (RU ⇄ EN)");
  fillLangSelect($("set-trtarget"), false);
  fillLangSelect($("set-trsecondary"), false);
  await refreshState();
  $("sum-length").value = state.settings.summary_length;
  $("sum-lang").value = state.settings.summary_language;
  $("tr-target").value = "auto";
  if (state.downloading) { downloadActive = true; renderSetup(); }
  countText($("sum-input"), $("sum-count"));
  countText($("tr-input"), $("tr-count"));
})();
