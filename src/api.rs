//! The client: REST and GraphQL over one blocking HTTP agent, run on
//! gpui's background executor so the UI thread never waits on the
//! network.
//!
//! Screens speak GitHub's API. A client for a GitLab account answers those
//! same requests from GitLab's API (see [`crate::gitlab`]); paths under
//! `/api/v4/` go to GitLab as they are, for the pages only GitLab has.
//!
//! Everything comes back as `serde_json::Value`; see [`crate::json`] for
//! how it is read.

use crate::forge::{Account, Forge};
use anyhow::{anyhow, Context as _, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const API: &str = "https://api.github.com";
/// GitHub's site, for the pages only GitHub has. Pages every forge has use
/// [`crate::forge::web`].
pub const WEB: &str = "https://github.com";

/// What a request answered with.
pub struct Reply {
    pub status: u16,
    pub body: String,
    /// The OAuth scopes the token carries, from `X-OAuth-Scopes`.
    pub scopes: Option<String>,
    /// The scopes the endpoint would take, from `X-Accepted-OAuth-Scopes`.
    pub accepted: Option<String>,
    /// GitLab's `X-Total`, when it counts a list.
    pub total: Option<u64>,
}

/// An error answer, keeping its status for whoever needs to tell a 404
/// from the rest. It reads as the explanation alone.
#[derive(Debug)]
pub struct Status {
    pub code: u16,
    pub message: String,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Status {}

/// The status an error carries, if it came from an answer.
pub fn status_of(error: &anyhow::Error) -> Option<u16> {
    error.downcast_ref::<Status>().map(|s| s.code)
}

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    token: Arc<str>,
    pub forge: Forge,
    /// The REST API's root: `https://api.github.com`, `https://host/api/v4`.
    api: Arc<str>,
    /// The site: `https://github.com`, `https://gitlab.example.com`.
    pub web: Arc<str>,
    /// What the GitLab translation keeps between requests.
    pub memo: Arc<crate::gitlab::Memo>,
}

impl Client {
    pub fn new(account: &Account) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // Error bodies carry the forge's explanation; read them rather
            // than getting a bare status code back.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(90)))
            .user_agent("visualhub")
            .build()
            .into();
        Client {
            agent,
            token: account.token.trim().into(),
            forge: account.forge,
            api: account.api().into(),
            web: account.web().into(),
            memo: Arc::default(),
        }
    }

    pub fn is_gitlab(&self) -> bool {
        self.forge == Forge::GitLab
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if self.is_gitlab() && path.starts_with("/api/") {
            format!("{}{path}", self.web)
        } else {
            format!("{}{path}", self.api)
        }
    }

    /// Whether a path goes to GitLab as it is rather than translated.
    fn native(&self, path: &str) -> bool {
        path.starts_with("/api/v4/") || path.starts_with("http://") || path.starts_with("https://")
    }

    /// A request as the screens make it: to GitHub, or translated for
    /// GitLab and answered as GitHub would.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        accept: Option<&str>,
    ) -> Result<Reply> {
        if self.is_gitlab() && !self.native(path) {
            return crate::gitlab::send(self, method, path, body);
        }
        self.raw(method, path, body, accept)
    }

    /// A request to the forge's own API, as it is. `accept` overrides the
    /// JSON media type for the endpoints that answer raw text or diffs.
    pub fn raw(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        accept: Option<&str>,
    ) -> Result<Reply> {
        let url = self.url(path);
        let builder = ureq::http::Request::builder().method(method).uri(&url);
        let builder = match self.forge {
            Forge::GitHub => builder
                .header("Accept", accept.unwrap_or("application/vnd.github+json"))
                .header("X-GitHub-Api-Version", "2022-11-28"),
            Forge::GitLab => builder.header("Accept", accept.unwrap_or("application/json")),
        };
        // Only the forge gets the token; redirects to blob storage (logs,
        // archives) are signed URLs and ureq drops auth headers on them.
        let builder = match self.forge {
            _ if self.token.is_empty() => builder,
            Forge::GitHub if url.starts_with(&*self.api) => {
                builder.header("Authorization", format!("Bearer {}", self.token))
            }
            // Bearer, not PRIVATE-TOKEN: GitLab takes access tokens either
            // way, but OAuth tokens (the GitLab CLI's web sign-in) only so.
            Forge::GitLab if url.starts_with(&*self.web) => {
                builder.header("Authorization", format!("Bearer {}", self.token))
            }
            _ => builder,
        };
        let mut response = match body {
            Some(body) => {
                let bytes = serde_json::to_vec(body)?;
                let request = builder
                    .header("Content-Type", "application/json")
                    .body(bytes)?;
                self.agent.run(request)
            }
            None if matches!(method, "PUT" | "POST" | "PATCH") => {
                // GitHub wants a Content-Length on bodiless writes
                // (PUT /user/starred/...); an empty body gives it one.
                let request = builder.header("Content-Length", "0").body(Vec::<u8>::new())?;
                self.agent.run(request)
            }
            None => self.agent.run(builder.body(())?),
        }
        .with_context(|| format!("{method} {path}"))?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let scopes = header("x-oauth-scopes");
        let accepted = header("x-accepted-oauth-scopes");
        let total = header("x-total").and_then(|t| t.parse().ok());
        let body = response
            .body_mut()
            .with_config()
            .limit(128 * 1024 * 1024)
            .read_to_string()
            .unwrap_or_default();
        Ok(Reply {
            status,
            body,
            scopes,
            accepted,
            total,
        })
    }

    /// The forge's own API as JSON: an error status becomes an `Err`
    /// carrying its message; an empty success body reads as `null`.
    pub fn raw_json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let reply = self.raw(method, path, body, None)?;
        reply_json(&reply, path)
    }

    /// The forge's own API as text.
    pub fn raw_text(&self, path: &str, accept: &str) -> Result<String> {
        let reply = self.raw("GET", path, None, Some(accept))?;
        if reply.status >= 400 {
            return Err(failure(&reply));
        }
        Ok(reply.body)
    }

    /// A JSON request; an error status becomes an `Err` carrying the
    /// forge's message. An empty success body reads as `null`.
    pub fn json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        if self.is_gitlab() && !self.native(path) {
            return crate::gitlab::rest(self, method, path, body);
        }
        self.raw_json(method, path, body)
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.json("GET", path, None)
    }

    /// Raw text: file contents, job logs, diffs.
    pub fn text(&self, path: &str, accept: &str) -> Result<String> {
        if self.is_gitlab() && !self.native(path) {
            return crate::gitlab::text(self, path);
        }
        self.raw_text(path, accept)
    }

    /// Whether a "check" endpoint answered 204 (yes) or 404 (no):
    /// `/user/starred/{owner}/{repo}`, `/user/following/{user}`.
    pub fn check(&self, path: &str) -> Result<bool> {
        if self.is_gitlab() && !self.native(path) {
            return crate::gitlab::check(self, path);
        }
        let reply = self.raw("GET", path, None, None)?;
        match reply.status {
            200..=299 => Ok(true),
            404 => Ok(false),
            _ => Err(failure(&reply)),
        }
    }

    /// A GraphQL query or mutation; GraphQL errors become an `Err`. On
    /// GitLab the few queries the screens send are answered from REST.
    pub fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        if self.is_gitlab() {
            return crate::gitlab::graphql(self, query, variables);
        }
        self.graphql_at("/graphql", query, variables)
    }

    /// GitLab's own GraphQL API.
    pub fn gitlab_graphql(&self, query: &str, variables: Value) -> Result<Value> {
        self.graphql_at("/api/graphql", query, variables)
    }

    fn graphql_at(&self, path: &str, query: &str, variables: Value) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables });
        let reply = self.raw_json("POST", path, Some(&body))?;
        if let Some(errors) = reply.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                let messages: Vec<String> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                    .map(str::to_string)
                    .collect();
                anyhow::bail!(messages.join("; "));
            }
        }
        Ok(reply.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Bytes from anywhere: avatars and images. Only a GitLab instance's
    /// own files get the token, for private projects' uploads and files.
    pub fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut request = self.agent.get(url);
        if self.is_gitlab() && url.starts_with(&*self.web) && !self.token.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.token));
        }
        let mut response = request.call()?;
        if response.status().as_u16() >= 400 {
            anyhow::bail!("{} fetching {url}", response.status());
        }
        Ok(response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_vec()?)
    }
}

/// An answer as JSON, or the error it carries.
pub fn reply_json(reply: &Reply, path: &str) -> Result<Value> {
    if reply.status >= 400 {
        return Err(failure(reply));
    }
    if reply.body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&reply.body).with_context(|| format!("decoding {path}"))
}

/// An error answer as an error that keeps its status.
pub fn failure(reply: &Reply) -> anyhow::Error {
    anyhow::Error::new(Status {
        code: reply.status,
        message: explain_reply(reply),
    })
}

/// An error reply as one readable line. GitHub answers 404 or 403 when a
/// classic token lacks the endpoint's scope, which says nothing useful, so
/// that case names the scope to add instead.
fn explain_reply(reply: &Reply) -> String {
    if let Some(scope) = missing_scope(reply) {
        return format!(
            "Your token is missing the \"{scope}\" scope this page needs. \
             If you signed in with the GitHub CLI, run `gh auth refresh -s {scope}` and reload; \
             otherwise sign in again with a token that has it."
        );
    }
    explain(reply.status, &reply.body)
}

/// The scope to ask for when a 403/404 is down to the token's scopes:
/// the endpoint names the ones it takes and the token has none of them.
/// Fine-grained tokens report no scopes, so they never match.
fn missing_scope(reply: &Reply) -> Option<String> {
    if !matches!(reply.status, 403 | 404) {
        return None;
    }
    let list = |s: &str| -> Vec<String> {
        s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
    };
    let have = list(reply.scopes.as_deref()?);
    let accepted = list(reply.accepted.as_deref()?);
    if have.is_empty() || accepted.iter().any(|s| have.contains(s)) {
        return None;
    }
    // GitHub lists the broadest scope first; it also covers the writes
    // the settings pages make.
    accepted.into_iter().next()
}

/// GitHub's error body as one readable line.
fn explain(status: u16, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let mut message = match parsed.as_ref().and_then(|v| v.get("message").or_else(|| v.get("error"))) {
        Some(Value::String(m)) => m.clone(),
        // GitLab's validation errors: `{"message": {"name": ["has already been taken"]}}`.
        Some(Value::Object(fields)) => fields
            .iter()
            .map(|(field, why)| match why {
                Value::Array(list) => format!("{field} {}", list.iter().filter_map(|w| w.as_str()).collect::<Vec<_>>().join(", ")),
                other => format!("{field} {}", other.as_str().unwrap_or("")),
            })
            .collect::<Vec<_>>()
            .join("; "),
        Some(Value::Array(list)) => list.iter().filter_map(|m| m.as_str()).collect::<Vec<_>>().join("; "),
        _ => String::new(),
    };
    if let Some(errors) = parsed
        .as_ref()
        .and_then(|v| v.get("errors"))
        .and_then(|e| e.as_array())
    {
        let details: Vec<String> = errors
            .iter()
            .map(|e| match e {
                Value::String(s) => s.clone(),
                other => other
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        let field = other.get("field").and_then(|f| f.as_str()).unwrap_or("");
                        let code = other.get("code").and_then(|f| f.as_str()).unwrap_or("");
                        format!("{field} {code}").trim().to_string()
                    }),
            })
            .filter(|s| !s.is_empty())
            .collect();
        if !details.is_empty() {
            message = format!("{message} ({})", details.join("; "));
        }
    }
    if message.is_empty() {
        message = body.chars().take(200).collect();
    }
    // GitLab's messages often lead with the status already ("401 Unauthorized").
    if message.starts_with(&status.to_string()) {
        message
    } else {
        format!("{status}: {message}")
    }
}

// ---------------------------------------------------------------------------
// Where accounts come from, and what is remembered between runs.

/// Where VisualHub keeps what it remembers between runs.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("visualhub"))
}

/// What an account's remembered things are filed under: the login alone
/// on github.com, as it always was, and the host too elsewhere.
fn account_file(prefix: &str, account: &Account, login: &str) -> String {
    match account.forge {
        Forge::GitHub => format!("{prefix}-{login}"),
        Forge::GitLab => format!("{prefix}-{}-{login}", account.host_name().replace([':', '/'], "_")),
    }
}

/// The repositories this account opened lately, newest first.
pub fn load_recent(account: &Account, login: &str) -> Vec<String> {
    config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join(account_file("recent", account, login))).ok())
        .map(|text| text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Remembering is best-effort: a failed write only costs the list.
pub fn save_recent(account: &Account, login: &str, repos: &[String]) {
    let Some(dir) = config_dir() else { return };
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(account_file("recent", account, login)), repos.join("\n"));
    }
}

/// Each repository's chosen merge method, as `merge.method:{repo}`
/// choices, from the last run.
pub fn load_merge_methods() -> Vec<(String, String)> {
    config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("merge-methods")).ok())
        .map(|text| {
            text.lines()
                .filter_map(|l| l.split_once('\t'))
                .map(|(repo, method)| (format!("merge.method:{repo}"), method.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Write back every `merge.method:` choice.
pub fn save_merge_methods(choices: &std::collections::HashMap<String, String>) {
    let Some(dir) = config_dir() else { return };
    let mut lines: Vec<String> = choices
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("merge.method:").map(|repo| format!("{repo}\t{v}")))
        .collect();
    lines.sort();
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("merge-methods"), lines.join("\n"));
    }
}

/// The accounts signed in to here, as `forge<TAB>host<TAB>token` lines.
/// A sign-in from before there were several accounts left a `token`
/// file with a GitHub token in it, which counts as the first.
pub fn saved_accounts() -> Vec<Account> {
    let Some(dir) = config_dir() else { return Vec::new() };
    let mut accounts: Vec<Account> = std::fs::read_to_string(dir.join("accounts"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let forge = Forge::from_key(parts.next()?)?;
            let (host, token) = (parts.next()?, parts.next()?);
            (!token.trim().is_empty()).then(|| Account::new(forge, host, token))
        })
        .collect();
    if let Ok(token) = std::fs::read_to_string(dir.join("token")) {
        let old = Account::new(Forge::GitHub, "github.com", &token);
        if !old.token.is_empty() && !accounts.contains(&old) {
            accounts.insert(0, old);
        }
    }
    accounts
}

fn write_accounts(accounts: &[Account]) -> Result<()> {
    let dir = config_dir().ok_or_else(|| anyhow!("no configuration directory"))?;
    std::fs::create_dir_all(&dir)?;
    let lines: Vec<String> = accounts
        .iter()
        .map(|a| format!("{}\t{}\t{}", a.forge.key(), a.host, a.token))
        .collect();
    std::fs::write(dir.join("accounts"), lines.join("\n"))?;
    // The accounts file has it now.
    let _ = std::fs::remove_file(dir.join("token"));
    Ok(())
}

/// Keep `account` for next time.
pub fn save_account(account: &Account) -> Result<()> {
    let mut accounts = saved_accounts();
    accounts.retain(|a| a != account);
    accounts.insert(0, account.clone());
    write_accounts(&accounts)
}

pub fn forget_account(account: &Account) {
    let mut accounts = saved_accounts();
    let before = accounts.len();
    accounts.retain(|a| a != account);
    if accounts.len() != before {
        let _ = write_accounts(&accounts);
    }
}

/// The account that was showing when the app last closed, by its key.
pub fn last_account() -> Option<String> {
    let text = std::fs::read_to_string(config_dir()?.join("active")).ok()?;
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

pub fn save_last_account(account: &Account) {
    let Some(dir) = config_dir() else { return };
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("active"), account.key());
    }
}

/// Every account this machine already has a token for: the ones signed
/// in to here; `GH_TOKEN`/`GITHUB_TOKEN` and the GitHub CLI's login;
/// `GITLAB_TOKEN` (for `GITLAB_HOST`, or gitlab.com) and the GitLab CLI's
/// logins.
pub fn discover_accounts() -> Vec<(Account, &'static str)> {
    let mut found: Vec<(Account, &'static str)> = Vec::new();
    let mut add = |account: Account, source: &'static str| {
        if !account.token.is_empty() && !found.iter().any(|(a, _)| *a == account) {
            found.push((account, source));
        }
    };
    for account in saved_accounts() {
        add(account, "saved sign-in");
    }
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(var) {
            add(Account::new(Forge::GitHub, "github.com", &token), "environment");
        }
    }
    if let Some(token) = gh_cli_token() {
        add(Account::new(Forge::GitHub, "github.com", &token), "GitHub CLI");
    }
    let env_host = ["GITLAB_HOST", "GL_HOST", "CI_SERVER_HOST"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|h| !h.trim().is_empty()));
    for var in ["GITLAB_TOKEN", "GL_TOKEN"] {
        if let Ok(token) = std::env::var(var) {
            add(Account::new(Forge::GitLab, env_host.as_deref().unwrap_or("gitlab.com"), &token), "environment");
        }
    }
    let mut hosts = glab_hosts();
    if let Some(host) = &env_host {
        hosts.push(crate::forge::normalize_host(host));
    }
    if hosts.is_empty() {
        hosts.push("gitlab.com".into());
    }
    hosts.dedup();
    for host in hosts {
        if let Some(token) = glab_cli_token(&host) {
            add(Account::new(Forge::GitLab, &host, &token), "GitLab CLI");
        }
    }
    found
}

/// A CLI's output, without a console window flashing up behind the app.
fn cli_output(program: PathBuf, args: &[&str]) -> Option<String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn gh_cli_token() -> Option<String> {
    cli_output(crate::cli::gh(), &["auth", "token"])
}

fn glab_cli_token(host: &str) -> Option<String> {
    cli_output(crate::cli::glab(), &["config", "get", "token", "--host", host])
}

/// The hosts the GitLab CLI is signed in to, from its config file's
/// `hosts:` map.
fn glab_hosts() -> Vec<String> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = std::env::var_os("GLAB_CONFIG_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(dir).join("glab-cli"));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(&home).join(".config/glab-cli"));
        dirs.push(PathBuf::from(&home).join("Library/Application Support/glab-cli"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("glab-cli"));
    }
    let Some(text) = dirs.iter().find_map(|d| std::fs::read_to_string(d.join("config.yml")).ok()) else {
        return Vec::new();
    };
    glab_config_hosts(&text)
}

/// The keys of the top-level `hosts:` map in glab's YAML config.
fn glab_config_hosts(yaml: &str) -> Vec<String> {
    let mut hosts = Vec::new();
    let mut in_hosts = false;
    let mut indent = None;
    for line in yaml.lines() {
        let text = line.trim_end();
        if text.trim().is_empty() || text.trim_start().starts_with('#') {
            continue;
        }
        let depth = text.len() - text.trim_start().len();
        if depth == 0 {
            in_hosts = text == "hosts:";
            continue;
        }
        if !in_hosts {
            continue;
        }
        let at = *indent.get_or_insert(depth);
        if depth == at {
            if let Some(host) = text.trim().strip_suffix(':') {
                hosts.push(crate::forge::normalize_host(host.trim_matches(['"', '\''])));
            }
        }
    }
    hosts
}

/// Seal a secret for GitHub Actions/Dependabot/Codespaces: libsodium's
/// sealed box against the repository's (or org's) base64 public key.
pub fn seal_secret(public_key_b64: &str, secret: &str) -> Result<String> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::STANDARD;
    let key = engine.decode(public_key_b64.trim())?;
    let key: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow!("the public key is not 32 bytes"))?;
    let key = crypto_box::PublicKey::from(key);
    let sealed = key
        .seal(&mut crypto_box::aead::OsRng, secret.as_bytes())
        .map_err(|_| anyhow!("could not encrypt the secret"))?;
    Ok(engine.encode(sealed))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_glab_hosts() {
        let yaml = "git_protocol: ssh\nhosts:\n    gitlab.com:\n        token: x\n        api_protocol: https\n    git.example.com:\n        token: y\ndisplay_hyperlinks: false\n";
        assert_eq!(super::glab_config_hosts(yaml), ["gitlab.com", "git.example.com"]);
    }
}
