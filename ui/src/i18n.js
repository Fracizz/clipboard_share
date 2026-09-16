export const DEFAULT_LOCALE = "en";

const dictionaries = {
  en: {
    general: "General",
    settings: "Settings",
    search: "Search settings",
    devices: "My devices",
    about: "About",
    generalDescription: "A little setup. A seamless workflow.",
    devicesDescription: "Your devices, working together.",
    pairingDescription: "One connection. A more connected workflow.",
    aboutDescriptionTitle: "A small utility for a smoother day.",
    clipboardSync: "Clipboard sync",
    autoSync: "Sync across devices",
    syncDescription: "Share your clipboard with paired devices on your local network.",
    pairedDevices: "Paired devices",
    manageDevices: "View and manage your trusted devices.",
    privacyNote: "Clipboard content travels directly between your devices, without a cloud relay.",
    preferences: "Preferences",
    language: "Language",
    languageDescription: "Choose the language that feels like home.",
    thisDevice: "This device",
    currentDevice: "Your current device",
    local: "LOCAL",
    firstTime: "Better together.",
    firstTimeDescription: "Pair another device to make copy and paste feel effortless.",
    addDevice: "Add device",
    footerNote: "Made for a more connected workflow.",
    localOnly: "Your clipboard. Your network.",
    devicesNote: "Devices stay paired until you remove them. Keep both devices on the same local network to sync.",
    emptyDescription: "Add your first device to start sharing your clipboard.",
    receivePair: "Pair from another device",
    createCode: "Create a pairing code",
    receiveDescription: "Enter this code on your other device to connect to this one.",
    yourCode: "Your pairing code",
    connectExisting: "Connect with a code",
    connectDescription: "Use the address and code from your other device.",
    connect: "Connect",
    sameNetwork: "Make sure both devices are on the same network.",
    aboutTagline: "Copy here. Paste there.",
    aboutDescription: "A simple clipboard companion for your local network. Pair your devices once, then keep your workflow moving.",
    noResults: "No matching settings",
    searchHint: "Try “language”, “device”, or “pair”.",
    searchResults: "Search results",
    searchDescription: "Find just the setting you need.",
    pairingFailed: "Pairing failed. Create a new code to try again.",
    startingPairing: "Starting pairing…",
    deviceCount: (count) => `${count} ${count === 1 ? "device" : "devices"}`,
    eyebrow: "LAN clipboard",
    running: "Running",
    stopped: "Stopped",
    device: "Device",
    name: "Name",
    id: "Device ID",
    ports: "Service / pairing port",
    portsValue: (listen, pair) => `${listen} / ${pair}`,
    startSync: "Start sync",
    stop: "Stop",
    peers: "Peers",
    noPeers: "No paired devices",
    unpair: "Unpair",
    unpaired: (name) => `Unpaired ${name}`,
    pair: "Pair a device",
    optionalCode: "Custom code (optional)",
    auto: "Generate automatically",
    waitForPair: "Create code",
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
    general: "常规",
    settings: "设置",
    search: "搜索设置",
    devices: "我的设备",
    about: "关于",
    generalDescription: "简单设置，让复制与粘贴自然衔接。",
    devicesDescription: "让你的设备，默契协作。",
    pairingDescription: "连接另一台设备，开启流畅的跨设备体验。",
    aboutDescriptionTitle: "小小工具，让日常更顺畅。",
    clipboardSync: "剪贴板同步",
    autoSync: "跨设备同步",
    syncDescription: "在同一局域网内，与已配对设备共享剪贴板。",
    pairedDevices: "已配对设备",
    manageDevices: "查看和管理与你连接的设备。",
    privacyNote: "剪贴板内容在设备之间直接传输，无需云端中转。",
    preferences: "偏好设置",
    language: "语言",
    languageDescription: "选择你习惯的界面语言。",
    thisDevice: "本机信息",
    currentDevice: "你正在使用的设备",
    local: "本机",
    firstTime: "连接，让工作更轻松。",
    firstTimeDescription: "添加另一台设备，体验无缝的复制与粘贴。",
    addDevice: "添加设备",
    footerNote: "让灵感，自由流转。",
    localOnly: "你的剪贴板，你的局域网。",
    devicesNote: "配对关系将一直保留，直到你主动移除。同步时，请确保设备处于同一局域网。",
    emptyDescription: "添加第一台设备，让剪贴板在设备间自由流转。",
    receivePair: "让其他设备连接本机",
    createCode: "创建配对码",
    receiveDescription: "在另一台设备上输入配对码，即可与本机建立连接。",
    yourCode: "你的配对码",
    connectExisting: "使用配对码连接",
    connectDescription: "输入另一台设备的地址和配对码。",
    connect: "连接设备",
    sameNetwork: "请确保两台设备处于同一局域网。",
    aboutTagline: "在这里复制，在那里粘贴。",
    aboutDescription: "轻巧的局域网剪贴板工具。一次配对，让复制与粘贴跨越设备，专注每一刻的创作。",
    noResults: "没有找到相关设置",
    searchHint: "试试搜索“语言”、“设备”或“配对”。",
    searchResults: "搜索结果",
    searchDescription: "快速找到你需要的设置。",
    pairingFailed: "配对未完成，请重新创建配对码后重试。",
    startingPairing: "正在启动配对…",
    deviceCount: (count) => `${count} 台设备`,
    eyebrow: "局域网剪贴板",
    running: "运行中",
    stopped: "已停止",
    device: "本机",
    name: "名称",
    id: "设备标识",
    ports: "同步 / 配对端口",
    portsValue: (listen, pair) => `${listen} / ${pair}`,
    startSync: "开始同步",
    stop: "停止",
    peers: "已配对",
    noPeers: "尚未配对设备",
    unpair: "解除配对",
    unpaired: (name) => `已解除配对：${name}`,
    pair: "设备配对",
    optionalCode: "自定义配对码（可选）",
    auto: "自动生成",
    waitForPair: "创建配对码",
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
  const systemLocale = navigator.language?.toLowerCase().startsWith("zh") ? "zh" : DEFAULT_LOCALE;
  return setLocale(saved === "zh" || saved === "en" ? saved : systemLocale);
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
  document.querySelectorAll("[data-i18n-aria-label]").forEach((el) => {
    el.setAttribute("aria-label", t(el.getAttribute("data-i18n-aria-label")));
  });
  document.querySelectorAll("[data-lang]").forEach((el) => {
    const lang = el.getAttribute("data-lang");
    el.classList.toggle("active", lang === locale);
  });
}
