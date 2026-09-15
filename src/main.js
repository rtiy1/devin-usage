// works in Tauri (window.__TAURI__) and in a plain browser (dev server /api/stats)
const getStats = window.__TAURI__
  ? () => window.__TAURI__.core.invoke("stats")
  : () => fetch("/api/stats").then((r) => r.json());

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const fmt = (n) => Math.round(n).toLocaleString("en-US");
const fmtS = (n) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "K" : String(Math.round(n)));
const dateStr = (ms) => new Date(ms).toLocaleDateString("sv");

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

function quotaCard(title, usedPct, resetAt) {
  const cls = usedPct >= 90 ? "bad" : usedPct >= 70 ? "warn" : "";
  return `<div class="card">
    <div class="v">${usedPct}<small>%</small></div>
    <div class="l">${title} · 已用</div>
    <div class="qtrack"><div class="qfill ${cls}" style="width:${Math.min(100, usedPct)}%"></div></div>
    <div class="sub">剩 <b>${100 - usedPct}%</b> · ${countdown(resetAt)}后重置</div>
  </div>`;
}

function statCard(v, l, sub = "") {
  return `<div class="card"><div class="v">${v}</div><div class="l">${l}</div>${sub ? `<div class="sub">${sub}</div>` : ""}</div>`;
}

function table(list, keyOf, extra = "") {
  if (!list.length) return '<div class="dim" style="padding:8px 4px">无数据</div>';
  const max = Math.max(...list.map((r) => r.total), 1);
  const rows = list
    .map(
      (r) => `<tr>
      <td class="name" title="${esc(keyOf(r))}">${esc(keyOf(r))}</td>
      <td class="num">${fmt(r.requests)}</td>
      <td class="num">${fmtS(r.input)}</td>
      <td class="num">${fmtS(r.cacheRead)}</td>
      <td class="num">${fmtS(r.output)}</td>
      <td class="num"><b>${fmt(r.total)}</b></td>
      <td style="width:110px"><div class="mbar" style="width:${Math.round((r.total / max) * 100)}%"></div></td>
    </tr>`
    )
    .join("");
  return `<table><thead><tr><th></th><th class="num">请求</th><th class="num">输入</th><th class="num">缓存</th><th class="num">输出</th><th class="num">合计</th><th></th></tr></thead><tbody>${rows}</tbody></table>`;
}

let last = null;
async function refresh() {
  try {
    last = await getStats();
    $("err").hidden = true;
    $("liveDot").classList.remove("off");
    render();
  } catch (e) {
    $("liveDot").classList.add("off");
    const el = $("err");
    el.hidden = false;
    el.textContent = "读取失败：" + e;
  }
}

function render() {
  const d = last;
  const q = d.quota;
  $("clock").textContent = new Date().toLocaleTimeString("zh-CN", { hour12: false });
  $("planBadge").textContent = q ? `${q.plan} · ${q.user}${q.live ? " · 实时" : " · 缓存"}` : "";

  $("quota").innerHTML = q
    ? quotaCard("今日配额", 100 - q.dailyRemainingPct, q.dailyResetAt) +
      quotaCard("本周配额", 100 - q.weeklyRemainingPct, q.weeklyResetAt) +
      statCard(dateStr(q.planEnd), "账期结束", `${dateStr(q.planStart)} 开始 · 剩 ${countdown(q.planEnd)}`)
    : statCard("—", "配额", "Devin 运行后自动出现");

  $("totals").innerHTML =
    statCard(fmtS(d.today.total), "今天", `${d.today.requests} 次请求`) +
    statCard(fmtS(d.week.total), "本周", `${d.week.requests} 次请求`) +
    statCard(fmtS(d.all.total), "总计", `${d.sessionCount} 个会话`) +
    statCard(fmtS(d.all.output), "输出 tokens", `输入 ${fmtS(d.all.input)} · 缓存 ${fmtS(d.all.cacheRead)}`);

  // last-14-days bar chart
  const days = d.byDay.slice(-14);
  const todayStr = new Date().toLocaleDateString("sv");
  const max = Math.max(...days.map((x) => x.total), 1);
  $("chart").innerHTML =
    `<div class="chart">` +
    days
      .map(
        (x) => `<div class="col" title="${x.day} · ${fmt(x.total)} tokens · ${x.requests} 请求">
        <div class="bar${x.day === todayStr ? " today" : ""}" style="height:${Math.max(3, Math.round((x.total / max) * 100))}%"></div>
        <div class="d">${x.day.slice(5)}</div></div>`
      )
      .join("") +
    `</div>`;

  $("models").innerHTML = table(d.todayByModel, (r) => r.model);
  $("sessions").innerHTML = table(
    d.bySession.slice(0, 10),
    (r) => `${r.session} · ${r.title ?? ""}`
  );
  $("foot").textContent = `${d.dbPath}`;
}

refresh();
setInterval(refresh, 2000);
setInterval(() => last && render(), 30_000); // countdowns stay fresh
