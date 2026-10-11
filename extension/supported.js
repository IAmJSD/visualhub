// Which pages VisualHub can open: the same answer as `route_for_url` in
// src/hub.rs and `gitlab_route`/`bitbucket_route` in src/forge.rs. Keep
// them in step, or the button lights up on pages the app turns away.
// And how to tell a self-hosted forge by its page.

(function () {
  // GitHub's first path segments that aren't an owner.
  const GITHUB_RESERVED = [
    "settings", "notifications", "pulls", "issues", "marketplace", "explore",
    "topics", "sponsors", "features", "login", "orgs", "apps", "search",
    "codespaces", "new",
  ];

  const GITHUB_REPO_TABS = [
    "issues", "pulls", "discussions", "actions", "releases", "branches",
    "tags", "commits", "security", "pulse", "graphs", "settings", "projects",
  ];

  const isNum = (s) => /^\d+$/.test(s || "");

  // The path's segments, without the query, fragment or empty segments.
  function segments(path) {
    return path.split(/[?#]/)[0].split("/").filter(Boolean);
  }

  function github(path) {
    const parts = segments(path);
    if (parts.length === 0) return true;
    if (parts.length === 1) {
      return parts[0] === "new" || !GITHUB_RESERVED.includes(parts[0]);
    }
    if (parts[0] === "orgs") {
      return parts[2] === "teams" && parts.length >= 4;
    }
    if (GITHUB_RESERVED.includes(parts[0])) return false;
    const [, , ...rest] = parts;
    if (rest.length === 0) return true;
    const [page, id] = rest;
    switch (page) {
      case "issues":
        return true;
      case "pull":
        return isNum(id);
      case "discussions":
        return true;
      case "commit":
        return id !== undefined;
      case "actions":
        // A job opens on github.com, which streams its log.
        if (id === "runs" && rest[3] === "job") return false;
        if (id === "runs" && rest.length > 2) return isNum(rest[2]);
        return true;
      case "tree":
      case "blob":
        return id !== undefined;
      default:
        return GITHUB_REPO_TABS.includes(page);
    }
  }

  function gitlab(path) {
    const parts = segments(path);
    if (parts.length === 0) return true;
    const [first, second, third] = parts;
    if (first === "dashboard" || first === "search" || first === "users") return true;
    if (first === "projects" && second === "new") return true;
    if (first === "profile") return true;
    if (first === "-") {
      if (second === "snippets") return third !== undefined;
      return second === "user_settings" || second === "profile";
    }
    if (["groups", "explore", "admin", "help", "api", "oauth"].includes(first)) {
      return false;
    }
    const at = parts.indexOf("-");
    // A user, a group or a project.
    if (at === -1) return true;
    const [page, id, more] = parts.slice(at + 1);
    switch (page) {
      case "issues":
      case "work_items":
      case "boards":
      case "labels":
      case "milestones":
      case "commits":
      case "pipeline_schedules":
      case "environments":
      case "releases":
      case "tags":
      case "branches":
      case "settings":
      case "project_members":
      case "graphs":
      case "network":
      case "forks":
      case "starrers":
      case "packages":
      case "merge_requests":
      case "pipelines":
      case "jobs":
        return true;
      case "commit":
        return id !== undefined;
      case "compare":
        return (id || "").includes("...");
      case "tree":
      case "blob":
        return id !== undefined;
      default:
        return false;
    }
  }

  function bitbucket(path) {
    const parts = segments(path);
    if (parts.length === 0) return true;
    const [first, second, third] = parts;
    if (["dashboard", "account", "search"].includes(first)) return true;
    if (first === "repo" && second === "create") return true;
    if (first === "snippets") return third !== undefined;
    if (["site", "product", "support", "blog"].includes(first)) return false;
    if (parts.length === 1 || second === "workspace") return true;
    const [page, id] = parts.slice(2);
    switch (page) {
      case undefined:
      case "pull-requests":
      case "src":
      case "commits":
      case "pipelines":
      case "addon":
      case "downloads":
      case "admin":
        return page !== "addon" || id === "pipelines";
      case "branches":
        if (id !== "compare" || parts[4] === undefined) return true;
        try {
          return /[\r\n]/.test(decodeURIComponent(parts[4] || ""));
        } catch {
          return false;
        }
      default:
        return false;
    }
  }

  const PUBLIC = [
    { root: "https://github.com", forge: "github" },
    { root: "https://gitlab.com", forge: "gitlab" },
    { root: "https://bitbucket.org", forge: "bitbucket" },
  ];

  // What `url` is to VisualHub, given the self-hosted `sites` known so far
  // ({ root, forge } each; forge "gitlab", or "datacenter" for Bitbucket
  // Data Center). `state` is:
  //   "open"        VisualHub shows this page
  //   "elsewhere"   a forge's page VisualHub doesn't have
  //   "datacenter"  Bitbucket Data Center, which VisualHub can't sign in to
  //   "unknown"     not a forge VisualHub knows of (yet)
  // and `forge` and `root` say which site it's on.
  function classify(url, sites) {
    let parsed;
    try {
      parsed = new URL(url);
    } catch {
      return { state: "unknown" };
    }
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
      return { state: "unknown" };
    }
    const here = parsed.origin + parsed.pathname;
    // The longest root wins: a GitLab under /gitlab beats its host's.
    const site = [...PUBLIC, ...(sites || [])]
      .filter(({ root }) => {
        const r = root.replace(/\/+$/, "").toLowerCase();
        const h = here.toLowerCase();
        return h === r || h.startsWith(r + "/");
      })
      .sort((a, b) => b.root.length - a.root.length)[0];
    if (!site) return { state: "unknown" };
    const root = site.root.replace(/\/+$/, "");
    const path = (parsed.origin + parsed.pathname).slice(root.length) + parsed.search + parsed.hash;
    const page = { github, gitlab, bitbucket }[site.forge];
    const state = !page ? "datacenter" : page(path) ? "open" : "elsewhere";
    return { state, forge: site.forge, root };
  }

  // Run in a page: the self-hosted forge it belongs to, if any, by what
  // GitLab and Bitbucket Data Center put in every page. Self-contained, as
  // it's injected on its own.
  function probe() {
    const content = (selector) => document.querySelector(selector)?.getAttribute("content");
    const MANIFEST = "/-/manifest.json";
    const manifest = document.querySelector('link[rel="manifest"]')?.href || "";
    const gitlabManifest = manifest.endsWith(MANIFEST);
    if (
      content('meta[property="og:site_name"]') === "GitLab" ||
      (gitlabManifest && document.body?.dataset.page)
    ) {
      // GitLab may live under a path, which its manifest's address shows.
      const root = gitlabManifest ? manifest.slice(0, -MANIFEST.length) : location.origin;
      return { forge: "gitlab", root };
    }
    if (
      content('meta[name="application-name"]') === "Bitbucket" ||
      document.body?.classList.contains("bitbucket-theme")
    ) {
      return { forge: "datacenter", root: location.origin };
    }
    return null;
  }

  const api = { classify, probe };
  globalThis.visualhub = api;
  if (typeof module !== "undefined") module.exports = api;
})();
