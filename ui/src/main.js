import {
  applyStaticI18n,
  getLocale,
  loadLocale,
  setLocale,
  t,
} from "./i18n.js";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const els = {
  badge: document.getElementById("run-badge"),
  deviceName: document.getElementById("device-name"),
  deviceId: document.getElementById("device-id"),
  ports: document.getElementById("ports"),
  peers: document.getElementById("peers"),
  message: document.getElementById("message"),
  listenHint: document.getElementById("listen-hint"),
  listenCode: document.getElementById("listen-code"),
  peerAddress: document.getElementById("peer-address"),
  peerCode: document.getElementById("peer-code"),
  btnStart: document.getElementById("btn-start"),
  btnStop: document.getElementById("btn-stop"),
  btnListen: document.getElementById("btn-listen"),
  btnConnect: document.getElementById("btn-connect"),
};

let lastStatus = null;

function setMessage(text, kind = "") {
  els.message.textContent = text || "";
  els.message.className = `message${kind ? ` ${kind}` : ""}`;
}

function shortId(id) {
  return String(id).slice(0, 8);
}

function renderStatus(status) {
  lastStatus = status;
  els.badge.textContent = status.running ? t("running") : t("stopped");
  els.badge.className = `badge ${status.running ? "running" : "stopped"}`;
  els.deviceName.textContent = status.device_name;
  els.deviceId.textContent = status.device_id;
  els.ports.textContent = t("portsValue", status.listen_port, status.pairing_port);

  els.peers.innerHTML = "";
  if (!status.peers.length) {
    const empty = document.createElement("li");
    empty.className = "empty";
    empty.textContent = t("noPeers");
    els.peers.appendChild(empty);
    return;
  }

  for (const peer of status.peers) {
    const item = document.createElement("li");
    item.innerHTML = `
      <div class="name">${peer.device_name}</div>
      <div class="addr">${peer.address}</div>
      <div class="id mono">${shortId(peer.device_id)}…</div>
    `;
    const unpairBtn = document.createElement("button");
    unpairBtn.type = "button";
    unpairBtn.textContent = t("unpair");
    unpairBtn.addEventListener("click", async () => {
      try {
        const next = await invoke("unpair", { deviceId: peer.device_id });
        renderStatus(next);
        setMessage(t("unpaired", peer.device_name), "ok");
      } catch (error) {
        setMessage(String(error), "error");
      }
    });
    item.appendChild(unpairBtn);
    els.peers.appendChild(item);
  }
}

async function refresh() {
  try {
    const status = await invoke("get_status");
    renderStatus(status);
  } catch (error) {
    setMessage(String(error), "error");
  }
}

async function withBusy(button, work) {
  button.disabled = true;
  try {
    await work();
  } finally {
    button.disabled = false;
  }
}

async function applyLocale(next) {
  setLocale(next);
  applyStaticI18n();
  if (lastStatus) {
    renderStatus(lastStatus);
  }
  try {
    await invoke("set_locale", { locale: getLocale() });
  } catch {
    // tray sync is best-effort
  }
}

els.btnStart.addEventListener("click", () =>
  withBusy(els.btnStart, async () => {
    try {
      const status = await invoke("start_sync");
      renderStatus(status);
      setMessage(t("syncStarted"), "ok");
    } catch (error) {
      setMessage(String(error), "error");
      await refresh();
    }
  }),
);

els.btnStop.addEventListener("click", () =>
  withBusy(els.btnStop, async () => {
    try {
      const status = await invoke("stop_sync");
      renderStatus(status);
      setMessage(t("syncStopped"), "ok");
    } catch (error) {
      setMessage(String(error), "error");
      await refresh();
    }
  }),
);

els.btnListen.addEventListener("click", () =>
  withBusy(els.btnListen, async () => {
    const code = els.listenCode.value.trim() || null;
    els.listenHint.textContent = code ? code : "……";
    setMessage(t("waitingPeer"));
    try {
      const used = await invoke("pair_listen", { code });
      els.listenHint.textContent = used;
      setMessage(t("pairingComplete"), "ok");
      await refresh();
    } catch (error) {
      setMessage(String(error), "error");
    }
  }),
);

els.btnConnect.addEventListener("click", () =>
  withBusy(els.btnConnect, async () => {
    try {
      await invoke("pair_connect", {
        address: els.peerAddress.value.trim(),
        code: els.peerCode.value.trim(),
      });
      setMessage(t("pairingComplete"), "ok");
      await refresh();
    } catch (error) {
      setMessage(String(error), "error");
    }
  }),
);

document.querySelectorAll("[data-lang]").forEach((button) => {
  button.addEventListener("click", () => {
    applyLocale(button.getAttribute("data-lang"));
  });
});

await listen("pairing-started", (event) => {
  els.listenHint.textContent = event.payload;
  setMessage(t("waitingPeer"));
});

await listen("pairing-finished", () => {
  setMessage(t("pairingComplete"), "ok");
  refresh();
});

await listen("status-changed", () => {
  refresh();
});

loadLocale();
applyStaticI18n();
try {
  await invoke("set_locale", { locale: getLocale() });
} catch {
  // ignore
}
await refresh();
setInterval(refresh, 3000);
