// `node --test extension/supported.test.js` checks the button lights up
// where the app's own URL matching (src/hub.rs, src/forge.rs) takes the
// page.
const test = require("node:test");
const assert = require("node:assert");
const { classify, probe } = require("./supported.js");

const state = (url, sites) => classify(url, sites).state;
const yes = (url, sites) => assert.equal(state(url, sites), "open", url);
const no = (url, sites) => assert.notEqual(state(url, sites), "open", url);

test("GitHub", () => {
  yes("https://github.com/");
  yes("https://github.com/octocat");
  yes("https://github.com/rust-lang/rust");
  yes("https://github.com/rust-lang/rust/issues/42");
  yes("https://github.com/rust-lang/rust/pull/7/files#diff");
  yes("https://github.com/rust-lang/rust/blob/main/README.md");
  yes("https://github.com/rust-lang/rust/actions");
  yes("https://github.com/rust-lang/rust/actions/runs");
  yes("https://github.com/rust-lang/rust/actions/runs/12");
  yes("https://github.com/orgs/rust-lang/teams/core");
  yes("https://github.com/new");
  no("https://github.com/rust-lang/rust/actions/runs/12/job/3");
  no("https://github.com/rust-lang/rust/pull/new");
  no("https://github.com/rust-lang/rust/wiki");
  no("https://github.com/settings/profile");
  no("https://github.com/notifications");
  no("https://github.com/orgs/rust-lang/people");
  no("https://gist.github.com/octocat");
  no("https://example.com/a");
  no("chrome://extensions");
});

test("GitLab", () => {
  yes("https://gitlab.com/");
  yes("https://gitlab.com/gitlab-org/gitlab");
  yes("https://gitlab.com/gitlab-org/sub/project/-/merge_requests/12/diffs");
  yes("https://gitlab.com/gitlab-org/gitlab/-/compare/main...dev");
  yes("https://gitlab.com/-/snippets/9");
  yes("https://gitlab.com/projects/new");
  no("https://gitlab.com/groups/gitlab-org");
  no("https://gitlab.com/gitlab-org/gitlab/-/wikis/home");
  no("https://gitlab.com/gitlab-org/gitlab/-/compare");
});

test("self-hosted GitLab", () => {
  const sites = [{ root: "https://git.example.com", forge: "gitlab" }];
  assert.equal(state("https://git.example.com/a/b"), "unknown");
  yes("https://git.example.com/a/b", sites);
  yes("https://GIT.example.com/a/b/-/merge_requests/3", sites);
  assert.equal(state("https://git.example.com/a/b/-/wikis/x", sites), "elsewhere");
  assert.deepEqual(classify("https://git.example.com/a/b", sites), {
    state: "open",
    forge: "gitlab",
    root: "https://git.example.com",
  });
  // A GitLab under a path: its pages are read from there.
  const nested = [{ root: "https://example.com/gitlab", forge: "gitlab" }];
  yes("https://example.com/gitlab/a/b/-/issues/1", nested);
  assert.equal(classify("https://example.com/gitlab/a/b", nested).root, "https://example.com/gitlab");
  assert.equal(state("https://example.com/blog", nested), "unknown");
  assert.equal(state("https://example.com/gitlabby/a", nested), "unknown");
});

test("Bitbucket Data Center", () => {
  const sites = [{ root: "https://git.example.com/bitbucket", forge: "datacenter" }];
  const w = "https://git.example.com/bitbucket";
  yes(`${w}/projects/WID/repos/widget`, sites);
  yes(`${w}/projects/WID/repos/widget/browse/src/main.rs?at=refs%2Fheads%2Fmain`, sites);
  yes(`${w}/projects/WID/repos/widget/pull-requests/3/diff`, sites);
  yes(`${w}/users/ada/repos/notes/browse`, sites);
  yes(`${w}/projects/WID`, sites);
  yes(`${w}/dashboard`, sites);
  assert.equal(state(`${w}/projects/WID/repos/widget/wiki`, sites), "elsewhere");
  assert.equal(state(`${w}/admin/users`, sites), "elsewhere");
  assert.equal(classify(`${w}/projects/WID/repos/widget`, sites).forge, "datacenter");
  assert.equal(state("https://git.example.com/other/thing", sites), "unknown");
});

// `probe` reads the page it's run in: a stand-in document will do.
function page({ meta = {}, manifest, dataPage, bodyClass = "", origin = "https://x.test" }) {
  globalThis.location = { origin };
  globalThis.document = {
    querySelector(selector) {
      if (selector === 'link[rel="manifest"]') return manifest ? { href: manifest } : null;
      const key = Object.keys(meta).find((k) => selector.includes(`"${k}"`));
      return key ? { getAttribute: () => meta[key] } : null;
    },
    body: {
      dataset: dataPage ? { page: dataPage } : {},
      classList: { contains: (c) => bodyClass.split(" ").includes(c) },
    },
  };
  return probe();
}

test("telling forges by their pages", () => {
  assert.deepEqual(
    page({ meta: { "og:site_name": "GitLab" }, manifest: "https://git.test/-/manifest.json", dataPage: "projects:show" }),
    { forge: "gitlab", root: "https://git.test" }
  );
  assert.deepEqual(
    page({ manifest: "https://x.test/gitlab/-/manifest.json", dataPage: "projects:show" }),
    { forge: "gitlab", root: "https://x.test/gitlab" }
  );
  assert.deepEqual(page({ meta: { "application-name": "Bitbucket" }, bodyClass: " bitbucket-theme" }), {
    forge: "datacenter",
    root: "https://x.test",
  });
  globalThis.document.querySelector = (selector) =>
    selector === 'link[rel="search"]'
      ? { href: "https://x.test/bitbucket/plugins/servlet/opensearch-descriptor" }
      : selector.includes("application-name")
        ? { getAttribute: () => "Bitbucket" }
        : null;
  assert.deepEqual(probe(), { forge: "datacenter", root: "https://x.test/bitbucket" });
  assert.equal(page({ meta: { "og:site_name": "Example" }, manifest: "https://x.test/manifest.json" }), null);
});

test("Bitbucket", () => {
  yes("https://bitbucket.org/");
  yes("https://bitbucket.org/acme");
  yes("https://bitbucket.org/acme/widget/pull-requests/3/diff");
  yes("https://bitbucket.org/acme/widget/src/main/lib/");
  yes("https://bitbucket.org/acme/widget/addon/pipelines/home");
  yes("https://bitbucket.org/acme/widget/branches/compare/dev%0Dmain");
  no("https://bitbucket.org/acme/widget/branches/compare/dev");
  no("https://bitbucket.org/acme/widget/wiki");
  no("https://bitbucket.org/product/pricing");
  no("https://bitbucket.org/snippets");
});
