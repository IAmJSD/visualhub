//! The GitHub client: REST and GraphQL over one blocking HTTP agent, run
//! on gpui's background executor so the UI thread never waits on the
//! network.
//!
//! Everything comes back as `serde_json::Value`; see [`crate::json`] for
//! how it is read.

use anyhow::{anyhow, bail, Context as _, Result};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub const API: &str = "https://api.github.com";
pub const WEB: &str = "https://github.com";

/// What a request answered with.
pub struct Reply {
    pub status: u16,
    pub body: String,
    /// The OAuth scopes the token carries, from `X-OAuth-Scopes`.
    pub scopes: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    token: Arc<str>,
}

impl Client {
    pub fn new(token: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            // Error bodies carry GitHub's explanation; read them rather
            // than getting a bare status code back.
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(90)))
            .user_agent("visualhub")
            .build()
            .into();
        Client {
            agent,
            token: token.trim().into(),
        }
    }

    fn url(path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{API}{path}")
        }
    }

    /// A request with GitHub's headers. `accept` overrides the JSON media
    /// type for the endpoints that answer raw text or diffs.
    pub fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        accept: Option<&str>,
    ) -> Result<Reply> {
        let url = Self::url(path);
        let builder = ureq::http::Request::builder()
            .method(method)
            .uri(&url)
            .header("Accept", accept.unwrap_or("application/vnd.github+json"))
            .header("X-GitHub-Api-Version", "2022-11-28");
        // Only GitHub gets the token; redirects to blob storage (logs,
        // archives) are signed URLs and ureq drops auth headers on them.
        let builder = if url.starts_with(API) && !self.token.is_empty() {
            builder.header("Authorization", format!("Bearer {}", self.token))
        } else {
            builder
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
        let scopes = response
            .headers()
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
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
        })
    }

    /// A JSON request; an error status becomes an `Err` carrying GitHub's
    /// message. An empty success body reads as `null`.
    pub fn json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let reply = self.send(method, path, body, None)?;
        if reply.status >= 400 {
            bail!(explain(reply.status, &reply.body));
        }
        if reply.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&reply.body).with_context(|| format!("decoding {path}"))
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.json("GET", path, None)
    }

    /// Raw text: file contents, job logs, diffs.
    pub fn text(&self, path: &str, accept: &str) -> Result<String> {
        let reply = self.send("GET", path, None, Some(accept))?;
        if reply.status >= 400 {
            bail!(explain(reply.status, &reply.body));
        }
        Ok(reply.body)
    }

    /// Whether a "check" endpoint answered 204 (yes) or 404 (no):
    /// `/user/starred/{owner}/{repo}`, `/user/following/{user}`.
    pub fn check(&self, path: &str) -> Result<bool> {
        let reply = self.send("GET", path, None, None)?;
        match reply.status {
            200..=299 => Ok(true),
            404 => Ok(false),
            status => bail!(explain(status, &reply.body)),
        }
    }

    /// A GraphQL query or mutation; GraphQL errors become an `Err`.
    pub fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables });
        let reply = self.json("POST", "/graphql", Some(&body))?;
        if let Some(errors) = reply.get("errors").and_then(|e| e.as_array()) {
            if !errors.is_empty() {
                let messages: Vec<String> = errors
                    .iter()
                    .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                    .map(str::to_string)
                    .collect();
                bail!(messages.join("; "));
            }
        }
        Ok(reply.get("data").cloned().unwrap_or(Value::Null))
    }

    /// Bytes from anywhere, unauthenticated: avatars and images.
    pub fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let mut response = self.agent.get(url).call()?;
        if response.status().as_u16() >= 400 {
            bail!("{} fetching {url}", response.status());
        }
        Ok(response
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_vec()?)
    }
}

/// GitHub's error body as one readable line.
fn explain(status: u16, body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let mut message = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
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
    format!("{status}: {message}")
}

// ---------------------------------------------------------------------------
// Where a token comes from.

/// Where the signed-in token is kept between runs.
pub fn token_file() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("visualhub").join("token"))
}

pub fn save_token(token: &str) -> Result<()> {
    let path = token_file().ok_or_else(|| anyhow!("no configuration directory"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, token.trim())?;
    Ok(())
}

pub fn forget_token() {
    if let Some(path) = token_file() {
        let _ = std::fs::remove_file(path);
    }
}

/// Every token this machine already has, best first: the one saved by a
/// previous sign-in, `GH_TOKEN`/`GITHUB_TOKEN`, then the GitHub CLI's.
pub fn discover_tokens() -> Vec<(String, &'static str)> {
    let mut found = Vec::new();
    if let Some(saved) = token_file().and_then(|p| std::fs::read_to_string(p).ok()) {
        if !saved.trim().is_empty() {
            found.push((saved.trim().to_string(), "saved sign-in"));
        }
    }
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(token) = std::env::var(var) {
            if !token.trim().is_empty() {
                found.push((token.trim().to_string(), "environment"));
            }
        }
    }
    if let Some(token) = gh_cli_token() {
        found.push((token, "GitHub CLI"));
    }
    found
}

fn gh_cli_token() -> Option<String> {
    let mut command = std::process::Command::new("gh");
    command.args(["auth", "token"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        // CREATE_NO_WINDOW: no console flashing up behind the app.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!token.is_empty()).then_some(token)
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
