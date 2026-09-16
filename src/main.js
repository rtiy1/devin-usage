// works in Tauri (window.__TAURI__) and in a plain browser (dev server /api/*)
const isTauri = !!window.__TAURI__;
const getStats = isTauri
  ? () => window.__TAURI__.core.invoke("stats")
  : () => fetch("/api/stats").then((r) => r.json());
// tauri command name -> dev-server endpoint (where they differ from a simple _ -> / map)
const API_PATH = { auth_sync_devin: "auth/sync_devin" };
const api = (cmd, args) =>
  isTauri
    ? window.__TAURI__.core.invoke(cmd, args ?? {})
    : fetch("/api/" + (API_PATH[cmd] ?? cmd.replaceAll("_", "/")), args
        ? { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(args) }
        : undefined).then((r) => r.json());

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const fmt = (n) => Math.round(n).toLocaleString("en-US");
const fmtS = (n) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "K" : String(Math.round(n)));
const dateStr = (ms) => new Date(ms).toLocaleDateString("sv");
const timeStr = (ms) => new Date(ms).toLocaleTimeString("zh-CN", { hour12: false, hour: "2-digit", minute: "2-digit" });

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

// inline icons (lucide-style, stroke=currentColor)
const IC = {
  user: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="8" r="4"/><path d="M4 21c0-4 3.6-7 8-7s8 3 8 7"/></svg>',
  zap: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M13 2 3 14h7l-1 8 11-13h-7l1-7z"/></svg>',
  msg: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 12a8 8 0 0 1-8 8H4l2-3a8 8 0 1 1 15-5z"/></svg>',
  layers: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="m12 2 9 5-9 5-9-5 9-5z"/><path d="m3 12 9 5 9-5"/><path d="m3 17 9 5 9-5"/></svg>',
  code: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="m8 8-5 4 5 4M16 8l5 4-5 4"/></svg>',
  cal: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="5" width="18" height="16" rx="2"/><path d="M8 3v4M16 3v4M3 10h18"/></svg>',
  gauge: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 15a3 3 0 1 0 0-6"/><path d="M12 9V4"/><path d="M5 19a9 9 0 1 1 14 0"/></svg>',
  swap: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M8 3 4 7l4 4M4 7h16M16 21l4-4-4-4M20 17H4"/></svg>',
  refresh: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 12a9 9 0 1 1-3-6.7"/><path d="M21 3v6h-6"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 6h18M8 6V4h8v2M6 6l1 15h10l1-15"/></svg>',
  share: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M12 3v12M7 8l5-5 5 5"/><path d="M5 15v5h14v-5"/></svg>',
};

// remaining-pct row: label | pct right | thin bar | "剩 x% · R: <reset>"
function qrow(name, remainingPct, resetAt) {
  const p = remainingPct == null || remainingPct < 0 ? null : Math.min(100, remainingPct);
  const cls = p == null ? "" : p <= 10 ? "bad" : p <= 30 ? "warn" : "";
  return `<div class="qrow"><span class="qn">${name}</span><span class="qv ${cls}">${p == null ? "—" : p + "%"}</span></div>
    <div class="qtrack"><div class="qfill ${cls}" style="width:${p ?? 0}%"></div></div>
    <div class="qsub">${p == null ? "" : `剩 ${p}% · `}R: ${countdown(resetAt)}</div>`;
}

function statCard(v, l, sub = "", icon = IC.zap, tint = "ic-blue") {
  return `<div class="card"><div class="icw ${tint}">${icon}</div><div class="v">${v}</div><div class="l">${l}</div>${sub ? `<div class="sub">${sub}</div>` : ""}</div>`;
}

function table(list, keyOf) {
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

// ==================== tabs ====================
let activeTab = "dash";
document.querySelectorAll(".tab").forEach((t) => {
  t.onclick = () => {
    activeTab = t.dataset.tab;
    document.querySelectorAll(".tab").forEach((x) => x.classList.toggle("on", x === t));
    $("page-dash").hidden = activeTab !== "dash";
    $("page-accts").hidden = activeTab !== "accts";
    if (activeTab === "accts") loadAccounts();
  };
});

// ==================== dashboard ====================
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
  $("greet").textContent = q?.user ? `你好, ${q.user} 👋` : "Devin 用量";

  const qp = $("quota");
  qp.hidden = false;
  qp.innerHTML = q
    ? `<div class="ptitle"><span class="icw ic-green">${IC.zap}</span>配额
        <span class="dim" style="font-weight:400;font-size:11px">${esc(q.plan)} · ${q.live ? "实时" : "缓存"}</span>
        <span class="right dim" style="font-size:11px">账期 ${dateStr(q.planStart)} → ${dateStr(q.planEnd)}</span></div>
      ${qrow("今日配额", q.dailyRemainingPct, q.dailyResetAt)}
      ${qrow("本周配额", q.weeklyRemainingPct, q.weeklyResetAt)}`
    : `<div class="ptitle"><span class="icw ic-green">${IC.zap}</span>配额</div><div class="dim" style="padding:4px 0">登录后自动出现</div>`;

  // ---- online (network) usage — the same data windsurf.com/profile shows ----
  const o = d.online;
  const show = (...ids) => ids.forEach((id) => ($(id).hidden = false));
  $("online").hidden = !o;
  if (o) {
    show("onlineModels", "onlineModelsTitle", "onlineTools", "onlineToolsTitle");
    $("online").innerHTML =
      statCard(o.acuUsed.toFixed(2), "ACU 已用（30 天）", `${o.messagesSent} 条消息 · ${o.conversations} 个会话`, IC.gauge, "ic-cyan") +
      statCard(fmt(o.messagesSent), "消息发送（30 天）", "云端接口实时", IC.msg, "ic-blue") +
      statCard(o.conversations, "会话", `活跃 ${o.byDay.length} 天`, IC.layers, "ic-purple") +
      statCard(fmt(o.linesAccepted), "生成代码行", "全部接受", IC.code, "ic-green");

    const mmax = Math.max(...o.byModel.map((m) => m.messages), 1);
    $("onlineModels").innerHTML = `<table><tbody>` +
      o.byModel.slice(0, 10).map((m) => `<tr>
        <td class="name" title="${esc(m.model)}">${esc(m.model)}</td>
        <td class="num"><b>${m.messages}</b></td>
        <td class="num dim">${m.days} 天</td>
        <td style="width:110px"><div class="mbar" style="width:${Math.round((m.messages / mmax) * 100)}%"></div></td>
      </tr>`).join("") + `</tbody></table>`;

    const tmax = Math.max(...o.tools.map((t) => t.count), 1);
    $("onlineTools").innerHTML = `<table><tbody>` +
      o.tools.slice(0, 10).map((t) => `<tr>
        <td class="name">${esc(t.tool)}</td>
        <td class="num"><b>${fmt(t.count)}</b></td>
        <td style="width:110px"><div class="mbar" style="width:${Math.round((t.count / tmax) * 100)}%"></div></td>
      </tr>`).join("") + `</tbody></table>`;
  }

  $("totals").innerHTML =
    statCard(fmtS(d.today.total), "今天", `${d.today.requests} 次请求`, IC.zap, "ic-green") +
    statCard(fmtS(d.week.total), "本周", `${d.week.requests} 次请求`, IC.cal, "ic-cyan") +
    statCard(fmtS(d.all.total), "总计", `${d.sessionCount} 个会话`, IC.gauge, "ic-blue") +
    statCard(fmtS(d.all.output), "输出 tokens", `输入 ${fmtS(d.all.input)} · 缓存 ${fmtS(d.all.cacheRead)}`, IC.code, "ic-purple");

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

// ==================== 账号管理 ====================
let accts = [];
let acctFilter = "all";
let acctQuery = "";

function acctCard(a) {
  const s = a.status ?? {};
  const err = s.error;
  const title = a.builtin ? (s.user || a.label) : a.label;
  const sub = a.builtin ? "跟随 Devin 当前登录" : (s.email || "");
  const quotaRows = err
    ? `<div class="acct-err">⚠ ${esc(err)}</div>`
    : qrow("今日配额", s.dailyRemainingPct, s.dailyResetAt) +
      qrow("本周配额", s.weeklyRemainingPct, s.weeklyResetAt);
  return `<div class="acct-card${a.active ? " on" : ""}${err ? " haserr" : ""}">
    <div class="acct-head">
      <span class="acct-mail" title="${esc(sub)}">${esc(title)}</span>
      ${a.active ? '<span class="tag">当前</span>' : ""}
      ${err ? '<span class="tag errtag">错误</span>' : ""}
      ${s.plan ? `<span class="tag plan">${esc(s.plan)}</span>` : ""}
    </div>
    ${sub && !a.builtin ? `<div class="acct-sub dim">${esc(sub)}</div>` : ""}
    ${quotaRows}
    <div class="acct-foot">
      <span class="dim">${s.fetchedAt ? timeStr(s.fetchedAt) : ""}</span>
      <span class="acts">
        ${a.active ? "" : `<button class="icon" data-use="${a.id}" title="切换到此账号">${IC.swap}</button>`}
        <button class="icon" data-ref="${a.id}" title="刷新">${IC.refresh}</button>
        ${a.builtin ? "" : `<button class="icon" data-sync="${a.id}" title="同步登录态到 Devin（需重启 Devin）">${IC.share}</button>`}
        ${a.builtin ? "" : `<button class="icon danger" data-del="${a.id}" title="移除">${IC.trash}</button>`}
      </span>
    </div>
  </div>`;
}

function renderAccts() {
  const q = acctQuery.toLowerCase();
  const list = accts.filter((a) => {
    const s = a.status ?? {};
    if (acctFilter === "low" && !(s.dailyRemainingPct <= 20 || s.weeklyRemainingPct <= 20)) return false;
    if (q && !(a.label + (s.email ?? "") + (s.user ?? "")).toLowerCase().includes(q)) return false;
    return true;
  });
  $("acctGrid").innerHTML = list.map(acctCard).join("");
  $("acctEmpty").hidden = list.length > 0;
  $("acctGrid").querySelectorAll("[data-use]").forEach((b) => (b.onclick = () => useAcct(b.dataset.use)));
  $("acctGrid").querySelectorAll("[data-ref]").forEach((b) => (b.onclick = () => refreshAcct(b.dataset.ref)));
  $("acctGrid").querySelectorAll("[data-del]").forEach((b) => (b.onclick = () => removeAcct(b.dataset.del)));
  $("acctGrid").querySelectorAll("[data-sync]").forEach((b) => (b.onclick = () => syncAcct(b.dataset.sync)));
}

async function loadAccounts(bustId) {
  try {
    accts = await api(bustId ? "accounts_status" : "accounts_status", bustId ? { id: bustId } : undefined);
    renderAccts();
  } catch (e) {
    $("acctGrid").innerHTML = `<div class="err">账号列表加载失败：${esc(e?.message ?? e)}</div>`;
  }
}

async function useAcct(id) { await api("auth_use", { id }); loadAccounts(); refresh(); }
async function refreshAcct(id) { await loadAccounts(id); }
async function removeAcct(id) {
  if (!confirm("移除该账号的本地登录态？")) return;
  await api("auth_remove", { id });
  loadAccounts();
  refresh();
}
async function syncAcct(id) {
  const r = await api("auth_sync_devin", { id });
  alert(r?.note ?? r?.error ?? "完成");
}

// ---- add account (PKCE: loopback auto-capture, manual paste as fallback) ----
let pendingState = null;
let pollTimer = null;
function loginDone() {
  if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  $("loginPane").hidden = true;
  pendingState = null;
  loadAccounts();
  refresh();
}
function loginFail(e) {
  if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  const el = $("loginErr");
  el.hidden = false;
  el.textContent = "登录失败：" + (e?.message ?? e);
}
$("addAcct").onclick = async () => {
  try {
    const r = await api("auth_start", {});
    pendingState = r.state;
    $("loginUrl").href = r.url;
    $("loginHint").textContent = r.manual
      ? "登录完成后，把页面显示的代码粘贴到下面。"
      : "在浏览器里完成登录即可自动添加。若浏览器没有自动打开，点上面的链接；也可以手动粘贴代码。";
    if (isTauri) {
      try { await api("open_url", { url: r.url }); } catch {}
    } else {
      window.open(r.url, "_blank");
    }
    $("loginPane").hidden = false;
    $("loginErr").hidden = true;
    $("codeInput").value = "";
    $("codeInput").focus();
    if (!r.manual) {
      pollTimer = setInterval(async () => {
        if (!pendingState) return;
        try {
          const p = await api("auth_poll", { state: pendingState });
          if (p?.done) loginDone();
        } catch (e) { loginFail(e); }
      }, 1500);
    }
  } catch (e) {
    $("loginPane").hidden = false;
    const el = $("loginErr");
    el.hidden = false;
    el.textContent = "无法开始登录：" + (e?.message ?? e);
  }
};
$("loginCancel").onclick = () => {
  if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  if (pendingState) { try { api("auth_cancel", { state: pendingState }); } catch {} }
  $("loginPane").hidden = true;
  pendingState = null;
};
$("codeSubmit").onclick = async () => {
  const code = $("codeInput").value.trim();
  if (!code || !pendingState) return;
  try {
    const r = await api("auth_complete", { state: pendingState, code });
    if (r.error) throw new Error(r.error);
    loginDone();
  } catch (e) { loginFail(e); }
};

$("refreshAll").onclick = async () => {
  for (const a of accts) await loadAccounts(a.id);
};
$("acctSearch").oninput = (e) => { acctQuery = e.target.value; renderAccts(); };
document.querySelectorAll(".flt").forEach((f) => {
  f.onclick = () => {
    acctFilter = f.dataset.f;
    document.querySelectorAll(".flt").forEach((x) => x.classList.toggle("on", x === f));
    renderAccts();
  };
});

// ==================== boot ====================
refresh();
setInterval(refresh, 2000);
setInterval(() => last && render(), 30_000); // countdowns stay fresh
if (location.hash === "#accts") document.querySelector('[data-tab="accts"]').click();
