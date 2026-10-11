// The toolbar button: lit on a page VisualHub can open, grey elsewhere,
// and a click hands the page to the app through its visualhub:// link.
//
// github.com, gitlab.com and bitbucket.org are known by their addresses,
// as are the self-hosted GitLabs VisualHub was signed in to when it wrote
// this copy (hosts.js). Any other site is looked at once, when a page of
// it first loads: GitLab and Bitbucket Data Center say what they are in
// every page, and a site found that way is remembered. Nothing leaves
// the browser.

// Chromium runs this as a service worker, which loads its helpers here;
// Firefox lists them before this file in the manifest.
if (typeof importScripts === "function") {
  importScripts("hosts.js", "supported.js");
}

const ext = globalThis.browser ?? globalThis.chrome;
const { classify, probe } = globalThis.visualhub;

const ICONS = {
  on: { 16: "icons/on-16.png", 32: "icons/on-32.png", 48: "icons/on-48.png", 128: "icons/on-128.png" },
  off: { 16: "icons/off-16.png", 32: "icons/off-32.png", 48: "icons/off-48.png", 128: "icons/off-128.png" },
};

const TITLES = {
  open: "Open in VisualHub",
  elsewhere: "VisualHub doesn't have this page",
  unknown: "VisualHub can't open this page",
};

// Self-hosted forges found by looking at their pages, kept for good.
async function found() {
  const { sites = [] } = await ext.storage.local.get("sites");
  return sites;
}

async function sites() {
  return [...(globalThis.VISUALHUB_SITES || []), ...(await found())];
}

// Sites already looked at and found to be no forge. Kept for the
// session, which outlives the service worker.
const area = ext.storage.session ?? null;
const notForges = new Set();

async function isNotForge(origin) {
  if (notForges.has(origin)) return true;
  if (!area) return false;
  const { notForges: kept = [] } = await area.get("notForges");
  kept.forEach((o) => notForges.add(o));
  return notForges.has(origin);
}

async function markNotForge(origin) {
  notForges.add(origin);
  if (area) await area.set({ notForges: [...notForges] });
}

// Look at the tab's page: the site, null when it's no forge, or undefined
// when it can't be read (a browser page, a store page, a site the user
// hasn't let the extension see, a closed tab).
async function look(tabId) {
  try {
    const [result] = await ext.scripting.executeScript({ target: { tabId }, func: probe });
    return result?.result ?? null;
  } catch {
    return undefined;
  }
}

async function remember(site) {
  const kept = await found();
  if (!kept.some((s) => s.root === site.root)) {
    await ext.storage.local.set({ sites: [...kept, site] });
  }
}

// What the tab shows, looking at its page if its site is new. `loaded`
// says the page has finished loading, so there's something to look at;
// `again` looks even at a site already found to be no forge.
async function stateOf(tabId, url, loaded, again = false) {
  const known = classify(url || "", await sites());
  if (known.state !== "unknown" || !url || !loaded || !/^https?:/.test(url)) {
    return known;
  }
  const origin = new URL(url).origin;
  if (!again && (await isNotForge(origin))) return known;
  const site = await look(tabId);
  if (site === null) await markNotForge(origin);
  if (!site) return known;
  await remember(site);
  return classify(url, await sites());
}

async function paint(tabId, state) {
  try {
    await ext.action.setIcon({ tabId, path: state === "open" ? ICONS.on : ICONS.off });
    await ext.action.setTitle({ tabId, title: TITLES[state] });
  } catch {
    // The tab closed while this was on its way.
  }
}

async function refresh(tabId, url, loaded) {
  await paint(tabId, (await stateOf(tabId, url, loaded)).state);
}

async function refreshAll() {
  for (const tab of await ext.tabs.query({})) {
    refresh(tab.id, tab.url, tab.status === "complete");
  }
}

ext.tabs.onUpdated.addListener((tabId, change, tab) => {
  // GitHub and GitLab move between pages with the History API, which
  // reports a new URL without a load.
  if (change.url || change.status) refresh(tabId, tab.url, tab.status === "complete");
});
ext.tabs.onActivated.addListener(async ({ tabId }) => {
  try {
    const tab = await ext.tabs.get(tabId);
    refresh(tabId, tab.url, tab.status === "complete");
  } catch {}
});
ext.storage.onChanged.addListener((changes, where) => {
  if (where === "local" && changes.sites) refreshAll();
});
ext.runtime.onInstalled.addListener(refreshAll);
ext.runtime.onStartup.addListener(refreshAll);
refreshAll();

// The button stays clickable everywhere: where the extension may not read
// pages on its own (the user can narrow its site access), a click lets it
// look at this one.
ext.action.onClicked.addListener(async (tab) => {
  const { state, forge, root } = await stateOf(tab.id, tab.url, true, true);
  await paint(tab.id, state);
  if (state !== "open") return;
  // The browser asks before it hands a link to an app; the page stays
  // where it is. The forge and root let VisualHub offer to sign in to a
  // GitLab it hasn't seen.
  const query = new URLSearchParams({ url: tab.url, forge, root });
  ext.tabs.update(tab.id, { url: "visualhub://open?" + query });
});
