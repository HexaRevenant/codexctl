// ── Tauri IPC (compatible Tauri v2) ──────────────────────────────
// Prefer __TAURI_INTERNALS__ (always injected), fallback to __TAURI__ global
const _internals = window.__TAURI_INTERNALS__;
const _tauri = window.__TAURI__;

const invoke = _internals?.invoke
  || _tauri?.core?.invoke
  || _tauri?.invoke;

if (!invoke) {
  document.body.innerHTML = `
    <div style="padding:40px;text-align:center;color:var(--red);font-family:sans-serif">
      <h2>Error: Tauri API no disponible</h2>
      <p>URL actual: <code>${location.href}</code></p>
      <p style="color:var(--text2);font-size:0.85em">
        globals: ${Object.getOwnPropertyNames(window).filter(k => k.includes('TAURI') || k.includes('ipc')).join(', ') || 'ninguno'}
      </p>
    </div>`;
  throw new Error("Tauri API not available");
}

// ── Init ────────────────────────────────────────────────────────────

window.addEventListener("DOMContentLoaded", () => refreshAll());

// ── Commands ────────────────────────────────────────────────────────

async function refreshAll() {
  showLoading();
  try {
    const accounts = await invoke("list_accounts", { fetchQuota: true });
    const status = await invoke("get_status");
    renderTable(accounts, status);
    updateStatus(accounts, status);
  } catch (e) {
    showToast("Error: " + e, true);
    hideLoading();
  }
}

async function switchAccount(id) {
  try {
    const msg = await invoke("switch_account", { accountId: id });
    showToast(msg);
    await refreshAll();
  } catch (e) {
    showToast("Error: " + e, true);
  }
}

function showAddModal() {
  document.getElementById("nicknameInput").value = "";
  document.getElementById("addModal").classList.add("show");
  setTimeout(() => document.getElementById("nicknameInput").focus(), 100);
}

async function addAccount() {
  const nickname = document.getElementById("nicknameInput").value.trim();
  if (!nickname) return;
  closeModal("addModal");
  showToast("⏳ Abriendo navegador para login...");
  try {
    const msg = await invoke("add_account", { nickname });
    showToast(msg);
    await refreshAll();
  } catch (e) {
    showToast("Error: " + e, true);
  }
}

async function renameAccount(id) {
  const newName = prompt("Nuevo nombre:");
  if (!newName || !newName.trim()) return;
  try {
    const msg = await invoke("rename_account", { accountId: id, nickname: newName.trim() });
    showToast(msg);
    await refreshAll();
  } catch (e) {
    showToast("Error: " + e, true);
  }
}

async function removeAccount(id, nickname) {
  if (!confirm(`¿Eliminar "${nickname}"?`)) return;
  try {
    const msg = await invoke("remove_account", { accountId: id });
    showToast(msg);
    await refreshAll();
  } catch (e) {
    showToast("Error: " + e, true);
  }
}

// ── Render ──────────────────────────────────────────────────────────

function renderTable(accounts, status) {
  const tbody = document.getElementById("tbody");
  const table = document.getElementById("accountsTable");
  const loading = document.getElementById("loading");

  tbody.innerHTML = "";
  let okCount = 0;

  for (const acct of accounts) {
    const tr = document.createElement("tr");
    if (acct.is_active) tr.classList.add("active");

    // Quota colors
    const q5class = quotaClass(acct.quota_5h);
    const q7class = quotaClass(acct.quota_7d);

    tr.innerHTML = `
      <td><strong>${esc(acct.nickname)}</strong> ${acct.is_active ? '<span class="active-indicator">●</span>' : ''}</td>
      <td style="color:var(--text2)">${esc(acct.email)}</td>
      <td><span class="tag tag-${acct.plan_type}">${esc(acct.plan_type)}</span></td>
      <td class="${q5class}">${esc(acct.quota_5h)}</td>
      <td class="${q7class}">${esc(acct.quota_7d)}</td>
      <td style="color:var(--text2);font-size:0.85em">${esc(acct.reset_at)}</td>
      <td class="actions">
        <button onclick="switchAccount('${acct.id}')">Activar</button>
        <button onclick="renameAccount('${acct.id}')">✎</button>
        <button class="danger" onclick="removeAccount('${acct.id}','${esc(acct.nickname)}')">✕</button>
      </td>
    `;
    tbody.appendChild(tr);
    if (acct.quota_5h !== "100%" && acct.quota_7d !== "100%") okCount++;
  }

  // Update badge
  const badge = document.getElementById("activeBadge");
  if (status && status.nickname) {
    badge.textContent = `✓ ${status.nickname}`;
  } else {
    badge.textContent = "❌ Ninguna activa";
  }

  table.style.display = "";
  loading.style.display = "none";
}

function updateStatus(accounts, status) {
  const bar = document.getElementById("statusBar");
  const total = accounts.length;
  const ok = accounts.filter(a =>
    a.quota_5h !== "100%" && a.quota_5h !== "—" &&
    a.quota_7d !== "100%" && a.quota_7d !== "—"
  ).length;
  // Actually, count properly: accounts with "100%" are blocked
  const blocked = accounts.filter(a =>
    a.quota_5h === "100%" || a.quota_7d === "100%"
  ).length;
  bar.textContent = `📊 ${total} cuentas · ${blocked} al límite · ${total - blocked} con quota`;
}

// ── Helpers ─────────────────────────────────────────────────────────

function quotaClass(val) {
  if (val === "—" || val === "") return "";
  const num = parseFloat(val);
  if (isNaN(num)) return "";
  if (num >= 100) return "quota-full";
  if (num >= 80) return "quota-warn";
  return "quota-ok";
}

function showLoading() {
  document.getElementById("loading").style.display = "";
  document.getElementById("accountsTable").style.display = "none";
}

function hideLoading() {
  document.getElementById("loading").style.display = "none";
  document.getElementById("accountsTable").style.display = "";
}

function closeModal(id) {
  document.getElementById(id).classList.remove("show");
}

let toastTimer;

function showToast(msg, isError = false) {
  const t = document.getElementById("toast");
  t.textContent = msg;
  t.style.borderLeftColor = isError ? "var(--red)" : "var(--green)";
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("show"), 3000);
}

function esc(s) {
  const div = document.createElement("div");
  div.textContent = s;
  return div.innerHTML;
}
