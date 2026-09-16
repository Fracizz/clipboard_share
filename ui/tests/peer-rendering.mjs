// Run with Playwright installed: node ui/tests/peer-rendering.mjs
// Or set PLAYWRIGHT_MODULE to an existing Playwright module's absolute path.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { pathToFileURL } from "node:url";

const { chromium } = await import(process.env.PLAYWRIGHT_MODULE
  ? pathToFileURL(process.env.PLAYWRIGHT_MODULE).href : "playwright");
const ui = new URL("../", import.meta.url);
const config = JSON.parse(await readFile(new URL("src-tauri/tauri.conf.json", ui)));
const csp = config.app.security.csp;
assert.equal(typeof csp, "string");
assert.ok(!csp.includes("unsafe-inline") && !csp.includes("unsafe-eval"));
assert.ok(csp.includes("ipc:") && csp.includes("http://ipc.localhost"));

const peer = {
  device_name: '<img src=x onerror="window.injected=true"><script>window.injected=true</script>',
  address: '<svg data-injection-test="peer-address" onload="window.injected=true"></svg>',
  device_id: "<img>untrusted-device-id",
};
const status = {
  running: true, device_name: "Local device", device_id: "local-id",
  listen_port: 5000, pairing_port: 5001, peers: [peer],
};
const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost");
  const file = url.pathname === "/" ? "index.html" : url.pathname.slice(1);
  if (file.split("/").includes("..")) { res.writeHead(404).end(); return; }
  if (!["index.html", "main.js", "i18n.js", "styles.css"].includes(file)
      && !/^assets\/[a-zA-Z0-9_./-]+$/.test(file)) {
    res.writeHead(404).end();
    return;
  }
  try {
    // Both modes are deliberate: CSP must not hide a rendering vulnerability.
    if (url.searchParams.has("csp")) res.setHeader("Content-Security-Policy", csp);
    res.setHeader("Content-Type", file.endsWith(".js") ? "text/javascript"
      : file.endsWith(".css") ? "text/css"
      : file.endsWith(".svg") ? "image/svg+xml"
      : file.endsWith(".png") ? "image/png" : "text/html");
    const path = file === "main.js" && process.env.UI_MAIN_JS
      ? process.env.UI_MAIN_JS : new URL(`src/${file}`, ui);
    res.end(await readFile(path));
  } catch (error) {
    res.writeHead(500).end(String(error));
  }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
let browser;
try {
  browser = await chromium.launch({ headless: true });
  for (const policy of [false, true]) {
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    const errors = [];
    page.on("pageerror", error => errors.push(String(error)));
    await page.addInitScript(({ status }) => {
      window.calls = [];
      window.listeners = {};
      window.violations = [];
      document.addEventListener("securitypolicyviolation", event => {
        window.violations.push(event.violatedDirective);
      });
      window.__TAURI__ = {
        core: { invoke: async (command, args) => {
          window.calls.push({ command, args });
          if (command === "pair_listen") {
            return new Promise((resolve, reject) => {
              window.pendingPairing = { code: args.code || "012345", resolve, reject };
            });
          }
          if (command === "unpair") status.peers = [];
          // Native IPC serializes each response; do not mutate earlier UI snapshots.
          return structuredClone(status);
        } },
        event: { listen: async (name, callback) => {
          window.listeners[name] = callback;
          return () => {};
        } },
      };
    }, { status });
    await page.goto(`http://127.0.0.1:${server.address().port}/${policy ? "?csp" : ""}`);
    try {
      // New settings layouts keep the device list hidden until its page is selected.
      if (await page.locator('[data-page="devices"]').count()) {
        await page.locator('[data-page="devices"]').click();
      }
      await page.waitForFunction(() => document.querySelector("#peers")?.textContent.includes("untrusted")
        || document.querySelector("#peers")?.textContent.includes("window.injected"));
    } catch (error) {
      throw new Error(`Peer UI failed to initialize; page errors: ${errors.join("; ")}`, { cause: error });
    }
    const assertLiteralPeers = async () => {
      // Check content rather than element shape: cards may include safe decorative SVGs.
      const text = await page.locator("#peers").textContent();
      assert.ok(text.includes(peer.device_name), "Device name must remain literal text");
      assert.ok(text.includes(peer.address), "Address must remain literal text");
      assert.ok(text.includes(peer.device_id.slice(0, 8)), "Device ID must remain literal text");
      assert.equal(await page.locator('#peers script, #peers img[src="x"], #peers [data-injection-test]').count(), 0);
      assert.equal(await page.locator("#peers *").evaluateAll(nodes => nodes.some(node =>
        [...node.attributes].some(attribute => /^on/i.test(attribute.name)))), false);
      assert.equal(await page.evaluate(() => window.injected), undefined);
    };
    await assertLiteralPeers();
    // Stylesheet and module imports must load under the production policy.
    assert.equal(await page.evaluate(() => document.styleSheets.length), 1);
    assert.ok(await page.evaluate(() => document.styleSheets[0].cssRules.length > 0));
    assert.deepEqual(await page.evaluate(() => window.violations), []);
    if (await page.locator("#language-select").count()) {
      await page.locator('[data-page="general"]').click();
      await page.locator("#language-select").selectOption("zh");
      await page.locator('[data-page="devices"]').click();
    } else {
      await page.locator('[data-lang="zh"]').click();
    }
    await assertLiteralPeers();
    await page.locator("#peers button").click();
    await page.waitForFunction(() => window.calls.some(call => call.command === "unpair")
      && !document.querySelector("#peers").textContent.includes("window.injected"));
    assert.deepEqual(await page.evaluate(() => window.calls.find(call => call.command === "unpair")),
      { command: "unpair", args: { deviceId: peer.device_id } });
    assert.ok((await page.locator("#message").textContent()).includes(peer.device_name));
    assert.equal(await page.locator("#message *").count(), 0);

    await page.locator('[data-page="pairing"]').click();
    await page.locator("#btn-listen").click();
    await page.waitForFunction(() => window.pendingPairing);
    assert.equal(await page.locator("#btn-listen").isDisabled(), true);
    assert.equal(await page.locator("#listen-hint").textContent(), "······");
    // Submitting via Enter/programmatic submit must not start a second listener.
    await page.locator("#listen-form").evaluate(form => form.requestSubmit());
    assert.equal(await page.evaluate(() => window.calls.filter(c => c.command === "pair_listen").length), 1);
    const conflict = "配对端口 24818 已被占用，请关闭其他 ClipboardShare 实例或正在运行的 pair-listen 后重试";
    await page.evaluate(error => window.pendingPairing.reject(error), conflict);
    await page.waitForFunction(() => !document.querySelector("#btn-listen").disabled);
    assert.equal(await page.locator("#listen-hint").textContent(), "—");
    assert.equal(await page.locator("#pairing-error").isVisible(), true);
    assert.equal(await page.locator("#pairing-error").textContent(), conflict);

    // Retry with a leading-zero custom code, announcing it only after bind succeeds.
    await page.locator("#listen-code").fill("001234");
    await page.locator("#btn-listen").click();
    await page.waitForFunction(() => window.calls.filter(c => c.command === "pair_listen").length === 2);
    assert.equal(await page.locator("#pairing-error").isVisible(), false);
    assert.equal(await page.locator("#listen-hint").textContent(), "······");
    await page.evaluate(() => window.listeners["pairing-started"]({ payload: window.pendingPairing.code }));
    assert.equal(await page.locator("#listen-hint").textContent(), "001234");
    assert.equal(await page.locator("#btn-listen").isDisabled(), true);
    await page.evaluate(() => window.pendingPairing.resolve(window.pendingPairing.code));
    await page.waitForFunction(() => !document.querySelector("#btn-listen").disabled);
    assert.equal(await page.locator("#pairing-state").textContent(), "配对完成");
    assert.equal(await page.locator("#listen-code").isDisabled(), false);

    assert.deepEqual(errors, []);
    await page.close();
    console.log(`PASS: peer metadata, locale, unpair, pairing conflict/retry/readiness, duplicate submit, local assets (CSP ${policy ? "on" : "off"})`);
  }
} finally {
  await browser?.close();
  await new Promise(resolve => server.close(resolve));
}
