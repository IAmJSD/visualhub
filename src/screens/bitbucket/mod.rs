//! The pages only Bitbucket has, or has differently enough to want its
//! own: Pipelines (a pipeline and each step with its log), a
//! repository's settings, and people and workspaces. Most ask
//! Bitbucket's API directly (`/2.0/…`); all are drawn with the same
//! pieces as everything else.

pub mod people;
pub mod pipelines;
pub mod settings;

use crate::json::enc;

/// A repository's root in Bitbucket's API, as the client sends it
/// untranslated.
pub fn repo_api(repo: &str) -> String {
    let (ws, slug) = repo.split_once('/').unwrap_or((repo, ""));
    format!("/2.0/repositories/{}/{}", enc(ws), enc(slug))
}
