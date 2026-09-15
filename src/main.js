const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

const fmt = (n) => Math.round(n).toLocaleString("en-US");
const fmtS = (n) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "K" : String(Math.round(n)));

function countdown(ms) {
  if (!ms) return "?";
  const d = ms - Date.now();
  if (d <= 0) return "已到期";
  const h = Math.floor(d / 3600e3), m = Math.floor((d % 3600e3) / 60e3);
  const days = Math.floor(h / 24);
  if (days > 0) return `${days} 天 ${h % 24} 时`;
  if (h > 0) return `${h} 时 ${m} 分`;
  return `${m} 分`;
}
const dateStr = (ms) => new Date(ms).toLocaleDateString("sv");

function quotaCard(title, usedPct, resetAt) {
  const cls = usedPct >= 90 ? "fill-bad" : usedPct >= 70 ? "fill-warn" : "fill-ok";
  return `<div class="card"><div class="v">${usedPct}%</div>
    <div class="l">${title}已用</div>
    <div class="qbar"><div class="${cls}" style="width:${Math.min(100, usedPct)}%"></div></div>
    <div class="sub">${countdown(resetAt)}后重置</div></div>`;
}

function card(v, l, sub = "") {
  return `<div class="card"><div class="v">${v}</div><div class="l">${l}</div>${sub ? `<div class="sub">${sub}</div>` : ""}</div>`;
}

function table(list, keyOf, extra = () => "") {
  if (!list.length) return '<div class="dim">无数据</div>';
  const max = Math.max(...list.map((r) => r.total), 1);
  const rows = list
    .map(
      (r) => `<tr>
      <td class="name">${esc(keyOf(r))}</td>
      <td class="num">${fmt(r.requests)}</td>
      <td class="num">${fmtS(r.input)}</td>
      <td class="num">${fmtS(r.cacheRead)}</td>
      <td class="num">${fmtS(r.output)}</td>
      <td class="num"><b>${fmt(r.total)}</b></td>
      <td class="barcell"><div class="tbar" style="width:${Math.round((r.total / max) * 100)}%"></div></td>
      ${extra(r)}</tr>`
    )
    .join("");
  return `<table><tr><th></th><th class="num">请求</th><th class="num">输入</th><th class="num">缓存</th><th class="num">输出</th><th class="num">合计</th><th></th></tr>${rows}</table>`;
}

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

let last = null;
async function refresh() {
  try {
    last = await invoke("stats");
  } catch (e) {
    $("meta").textContent = "读取失败: " + e;
    return;
  }
  render();
}

function render() {
  const d = last;
  const q = d.quota;
  $("meta").textContent =
    `${new Date().toLocaleTimeString("zh-CN", { hour12: false })} · ${d.sessionCount} 个会话` +
    (q ? ` · 配额缓存 ${new Date(q.fetchedAt).toLocaleTimeString("zh-CN", { hour12: false })}` : "");

  $("quota").innerHTML = q
    ? `<div class="cards">` +
      quotaCard("今日配额", 100 - q.dailyRemainingPct, q.dailyResetAt) +
      quotaCard("本周配额", 100 - q.weeklyRemainingPct, q.weeklyResetAt) +
      card(`${esc(q.plan)}`, `套餐 · ${esc(q.user)}`, `账期 ${dateStr(q.planStart)} → ${dateStr(q.planEnd)}`) +
      `</div>`
    : `<div class="cards">${card("—", "配额数据", "Devin 运行后自动出现")}</div>`;

  $("totals").innerHTML =
    `<div class="cards">` +
    card(fmt(d.today.total), "今天 tokens", `${d.today.requests} req`) +
    card(fmt(d.week.total), "本周 tokens", `${d.week.requests} req`) +
    card(fmt(d.all.total), "全部 tokens", `${d.all.requests} req`) +
    card(fmtS(d.all.input), "全价输入", `缓存 ${fmtS(d.all.cacheRead)} · 输出 ${fmtS(d.all.output)}`) +
    `</div>`;

  $("models").innerHTML = table(d.todayByModel, (r) => r.model);
  $("days").innerHTML = table(d.byDay.slice(-14), (r) => r.day);
  $("sessions").innerHTML = table(
    d.bySession.slice(0, 12),
    (r) => `${r.session}  ${r.title ?? ""}`,
    (r) => `<td class="num dim">${r.createdAt ? dateStr(r.createdAt) : ""}</td>`
  );
  $("foot").textContent = d.dbPath;
}

$("autofresh").addEventListener("change", (e) => {
  clearInterval(timer);
  if (e.target.checked) timer = setInterval(refresh, 2000);
});

let timer = setInterval(refresh, 2000);
refresh();
setInterval(() => last && render(), 30_000); // keep countdowns fresh even if data unchanged
