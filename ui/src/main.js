import { applyStaticI18n, getLocale, loadLocale, setLocale, t } from "./i18n.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const els = {
  badge: $("run-badge"), deviceName: $("device-name"), deviceId: $("device-id"),
  ports: $("ports"), peers: $("peers"), message: $("message"),
  listenHint: $("listen-hint"), listenCode: $("listen-code"),
  peerAddress: $("peer-address"), peerCode: $("peer-code"),
  syncToggle: $("sync-toggle"), btnListen: $("btn-listen"), btnConnect: $("btn-connect"),
  search: $("settings-search"), language: $("language-select"),
};
const pages = {
  general: ["general", "generalDescription"],
  devices: ["devices", "devicesDescription"],
  pairing: ["pair", "pairingDescription"],
  about: ["about", "aboutDescriptionTitle"],
};
let activePage = "general";
let lastStatus = null;
let syncBusy = false;
let messageTimer;
let pairingState = "waitingPeer";

function setMessage(text, kind = "") {
  clearTimeout(messageTimer);
  els.message.textContent = text || "";
  els.message.className = `message${kind ? ` ${kind}` : ""}`;
  els.message.hidden = !text;
  if (text && kind === "ok") messageTimer = setTimeout(() => { els.message.hidden = true; }, 5000);
}

function showPage(page, focus = false) {
  if (!pages[page]) return;
  activePage = page;
  els.search.value = "";
  renderNavigation();
  if (focus) {
    $("page-title").tabIndex = -1;
    $("page-title").focus({ preventScroll: true });
    document.querySelector(".main-panel").scrollTop = 0;
  }
}

function renderNavigation() {
  const query = els.search.value.trim().toLocaleLowerCase();
  const searching = Boolean(query);
  $("page-title").textContent = t(searching ? "searchResults" : pages[activePage][0]);
  $("page-description").textContent = t(searching ? "searchDescription" : pages[activePage][1]);
  document.querySelectorAll("[data-page]").forEach((button) => {
    const selected = !searching && button.dataset.page === activePage;
    button.classList.toggle("active", selected);
    if (selected) button.setAttribute("aria-current", "page");
    else button.removeAttribute("aria-current");
  });
  let matches = 0;
  document.querySelectorAll("[data-view]").forEach((view) => {
    if (!searching) {
      view.hidden = view.dataset.view !== activePage;
      view.querySelectorAll("[data-searchable], .help-note").forEach((item) => { item.hidden = false; });
      return;
    }
    const groups = [...view.querySelectorAll("[data-searchable]")];
    let found = false;
    if (groups.length) {
      groups.forEach((group) => {
        group.hidden = !group.textContent.toLocaleLowerCase().includes(query);
        if (!group.hidden) found = true;
      });
    } else {
      found = `${t(pages[view.dataset.view][0])} ${view.textContent}`.toLocaleLowerCase().includes(query);
    }
    view.querySelectorAll(".help-note").forEach((item) => { item.hidden = true; });
    view.hidden = !found;
    if (found) matches++;
  });
  $("search-empty").hidden = !searching || matches > 0;
}

function renderPeers(peers) {
  els.peers.replaceChildren();
  if (!peers.length) {
    const empty = document.createElement("li");
    empty.className = "empty";
    const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", "#i-devices");
    icon.setAttribute("aria-hidden", "true");
    icon.appendChild(use);
    const heading = document.createElement("strong");
    heading.textContent = t("noPeers");
    const description = document.createElement("p");
    description.textContent = t("emptyDescription");
    const add = document.createElement("button");
    add.type = "button";
    add.textContent = t("addDevice");
    add.addEventListener("click", () => showPage("pairing", true));
    empty.append(icon, heading, description, add);
    els.peers.appendChild(empty);
    return;
  }
  for (const peer of peers) {
    const item = document.createElement("li");
    // Peer metadata comes from the network; never interpret it as HTML.
    for (const [className, text] of [
      ["name", peer.device_name], ["addr", peer.address],
      ["id mono", `${String(peer.device_id).slice(0, 8)}…`],
    ]) {
      const field = document.createElement("div");
      field.className = className;
      field.textContent = text;
      item.appendChild(field);
    }
    const unpairBtn = document.createElement("button");
    unpairBtn.type = "button";
    unpairBtn.textContent = t("unpair");
    unpairBtn.addEventListener("click", () => withBusy(unpairBtn, async () => {
      try {
        renderStatus(await invoke("unpair", { deviceId: peer.device_id }));
        setMessage(t("unpaired", peer.device_name), "ok");
      } catch (error) { setMessage(String(error), "error"); }
    }));
    item.appendChild(unpairBtn);
    els.peers.appendChild(item);
  }
}

function renderStatus(status, force = false) {
  const peersChanged = force || JSON.stringify(lastStatus?.peers) !== JSON.stringify(status.peers);
  lastStatus = status;
  els.badge.textContent = t(status.running ? "running" : "stopped");
  els.badge.className = `badge ${status.running ? "running" : "stopped"}`;
  $("sidebar-status").textContent = els.badge.textContent;
  $("sidebar-status-dot").classList.toggle("running", status.running);
  els.syncToggle.setAttribute("aria-checked", String(status.running));
  els.syncToggle.disabled = syncBusy;
  els.deviceName.textContent = status.device_name;
  els.deviceId.textContent = status.device_id;
  els.ports.textContent = t("portsValue", status.listen_port, status.pairing_port);
  $("peer-count").textContent = status.peers.length;
  $("paired-summary").textContent = t("deviceCount", status.peers.length);
  if (peersChanged) renderPeers(status.peers);
  if (els.search.value.trim()) renderNavigation();
}

async function refresh() {
  try { renderStatus(await invoke("get_status")); }
  catch (error) { setMessage(String(error), "error"); }
}

async function withBusy(button, work) {
  button.disabled = true;
  try { await work(); }
  finally { button.disabled = false; }
}

async function applyLocale(next) {
  setLocale(next);
  applyStaticI18n();
  els.language.value = getLocale();
  renderNavigation();
  if (lastStatus) renderStatus(lastStatus, true);
  $("pairing-state").textContent = t(pairingState);
  setMessage("");
  try { await invoke("set_locale", { locale: getLocale() }); }
  catch { /* Tray locale sync is best-effort. */ }
}

els.syncToggle.addEventListener("click", async () => {
  if (!lastStatus || syncBusy) return;
  syncBusy = true;
  const start = !lastStatus.running;
  await withBusy(els.syncToggle, async () => {
    try {
      renderStatus(await invoke(start ? "start_sync" : "stop_sync"));
      setMessage(t(start ? "syncStarted" : "syncStopped"), "ok");
    } catch (error) {
      setMessage(String(error), "error");
      await refresh();
    }
  });
  syncBusy = false;
});

$("listen-form").addEventListener("submit", (event) => {
  event.preventDefault();
  withBusy(els.btnListen, async () => {
    const code = els.listenCode.value.trim() || null;
    $("pairing-code-panel").hidden = false;
    els.listenHint.textContent = code || "······";
    pairingState = "waitingPeer";
    $("pairing-state").textContent = t(pairingState);
    els.listenCode.disabled = true;
    setMessage("");
    try {
      const used = await invoke("pair_listen", { code });
      els.listenHint.textContent = used;
      pairingState = "pairingComplete";
      setMessage(t(pairingState), "ok");
      await refresh();
    } catch (error) {
      pairingState = "pairingFailed";
      els.listenHint.textContent = "—";
      setMessage(String(error), "error");
    } finally {
      els.listenCode.disabled = false;
      $("pairing-state").textContent = t(pairingState);
    }
  });
});

$("connect-form").addEventListener("submit", (event) => {
  event.preventDefault();
  withBusy(els.btnConnect, async () => {
    try {
      await invoke("pair_connect", { address: els.peerAddress.value.trim(), code: els.peerCode.value.trim() });
      setMessage(t("pairingComplete"), "ok");
      els.peerCode.value = "";
      await refresh();
      showPage("devices", true);
    } catch (error) { setMessage(String(error), "error"); }
  });
});

document.querySelectorAll("[data-page], [data-go]").forEach((button) => {
  button.addEventListener("click", () => showPage(button.dataset.page || button.dataset.go, true));
});
els.language.addEventListener("change", () => applyLocale(els.language.value));
els.search.addEventListener("input", renderNavigation);
els.search.addEventListener("keydown", (event) => {
  if (event.key === "Escape") { els.search.value = ""; renderNavigation(); }
});
const isMac = /Mac/.test(navigator.platform);
document.querySelector("kbd").textContent = isMac ? "⌘ K" : "Ctrl K";
document.addEventListener("keydown", (event) => {
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
    event.preventDefault();
    els.search.focus();
    els.search.select();
  }
});

loadLocale();
applyStaticI18n();
els.language.value = getLocale();
renderNavigation();
try {
  await listen("pairing-started", (event) => {
    els.listenHint.textContent = event.payload;
    $("pairing-code-panel").hidden = false;
    pairingState = "waitingPeer";
    $("pairing-state").textContent = t(pairingState);
  });
  await listen("pairing-finished", () => {
    pairingState = "pairingComplete";
    $("pairing-state").textContent = t(pairingState);
    setMessage(t(pairingState), "ok");
    refresh();
  });
  await listen("status-changed", refresh);
} catch (error) { setMessage(String(error), "error"); }
try { await invoke("set_locale", { locale: getLocale() }); }
catch { /* Tray locale sync is best-effort. */ }
await refresh();
setInterval(refresh, 3000);
