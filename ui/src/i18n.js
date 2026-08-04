export const DEFAULT_LOCALE = "en";

const dictionaries = {
  en: {
    eyebrow: "LAN clipboard",
    running: "Running",
    stopped: "Stopped",
    device: "Device",
    name: "Name",
    id: "ID",
    ports: "Ports",
    portsValue: (listen, pair) => `${listen} / pair ${pair}`,
    startSync: "Start sync",
    stop: "Stop",
    peers: "Peers",
    noPeers: "No paired devices",
    unpair: "Unpair",
    unpaired: (name) => `Unpaired ${name}`,
    pair: "Pair",
    optionalCode: "Optional code",
    auto: "auto",
    waitForPair: "Wait for pair",
    peerAddress: "Peer address",
    pairingCode: "Pairing code",
    connectPeer: "Connect to peer",
    syncStarted: "Sync started",
    syncStopped: "Sync stopped",
    waitingPeer: "Waiting for peer to connect…",
    pairingComplete: "Pairing complete",
    langEn: "EN",
    langZh: "中文",
  },
  zh: {
    eyebrow: "局域网剪贴板",
    running: "运行中",
    stopped: "已停止",
    device: "本机",
    name: "名称",
    id: "ID",
    ports: "端口",
    portsValue: (listen, pair) => `${listen} / 配对 ${pair}`,
    startSync: "开始同步",
    stop: "停止",
    peers: "已配对",
    noPeers: "尚未配对设备",
    unpair: "解除配对",
    unpaired: (name) => `已解除配对：${name}`,
    pair: "配对",
    optionalCode: "可选配对码",
    auto: "自动生成",
    waitForPair: "等待配对",
    peerAddress: "对端地址",
    pairingCode: "配对码",
    connectPeer: "连接对端",
    syncStarted: "同步已启动",
    syncStopped: "同步已停止",
    waitingPeer: "正在等待对端连接…",
    pairingComplete: "配对完成",
    langEn: "EN",
    langZh: "中文",
  },
};

let locale = DEFAULT_LOCALE;

export function getLocale() {
  return locale;
}

export function t(key, ...args) {
  const dict = dictionaries[locale] || dictionaries.en;
  const value = dict[key] ?? dictionaries.en[key] ?? key;
  return typeof value === "function" ? value(...args) : value;
}

export function setLocale(next) {
  locale = next === "zh" ? "zh" : "en";
  try {
    localStorage.setItem("clipboard_share_locale", locale);
  } catch {
    // ignore storage failures
  }
  document.documentElement.lang = locale === "zh" ? "zh-CN" : "en";
  return locale;
}

export function loadLocale() {
  let saved = null;
  try {
    saved = localStorage.getItem("clipboard_share_locale");
  } catch {
    saved = null;
  }
  return setLocale(saved === "zh" ? "zh" : DEFAULT_LOCALE);
}

export function applyStaticI18n() {
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const key = el.getAttribute("data-i18n");
    if (key) {
      el.textContent = t(key);
    }
  });
  document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
    const key = el.getAttribute("data-i18n-placeholder");
    if (key) {
      el.setAttribute("placeholder", t(key));
    }
  });
  document.querySelectorAll("[data-lang]").forEach((el) => {
    const lang = el.getAttribute("data-lang");
    el.classList.toggle("active", lang === locale);
  });
}
