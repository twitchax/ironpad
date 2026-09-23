//! Notebook loaders shared by a full page and its `/embed/*` variant.
//!
//! Each one carries a policy that decides what a viewer receives: whether a
//! share's blob snapshots are used (PRD-0047) and when a manifest is withheld
//! (PRD-0061, PRD-0064). Two hand-kept copies of that policy is the shape
//! that has shipped bugs here before, so each exists once.

use ironpad_common::{IronpadNotebook, MutableNotebookAccess, ShareManifest};
use leptos::prelude::ServerFnError;

use crate::server_fns::{
    get_mutable_manifest, get_mutable_notebook, get_shared_manifest, get_shared_notebook,
};

/// An immutable share and its blob-snapshot manifest, for `/shared/{hash}`
/// and `/embed/shared/{hash}`.
///
/// A missing or failed manifest degrades to live compilation (PRD-0047); it
/// never fails the page.
pub(crate) async fn load_shared(
    hash: String,
) -> Result<(IronpadNotebook, Option<ShareManifest>), ServerFnError> {
    let notebook = get_shared_notebook(hash.clone()).await?;
    let manifest = get_shared_manifest(hash).await.unwrap_or(None);
    Ok((notebook, manifest))
}

/// A mutable share's reader access and, only when the reader may see the
/// notebook, its manifest, for `/mutable/{id}` and `/embed/mutable/{id}`.
///
/// The manifest is the hash list, so it is asked for only on `Found`; a
/// missing or degraded one falls back to live compilation.
pub(crate) async fn load_mutable(
    id: String,
) -> Result<(MutableNotebookAccess, Option<ShareManifest>), ServerFnError> {
    let access = get_mutable_notebook(id.clone()).await?;
    let manifest = if matches!(access, MutableNotebookAccess::Found(_)) {
        get_mutable_manifest(id).await.unwrap_or(None)
    } else {
        None
    };
    Ok((access, manifest))
}
