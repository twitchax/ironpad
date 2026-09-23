//! The storage classes a crawler-facing handler can load a notebook from.
//!
//! The OG card and the oEmbed provider both take a class plus an id and need
//! the notebook behind them. Before this module each wrote its own three-way
//! dispatch, and oEmbed matched a `&'static str` with a catch-all that sent any
//! unrecognised class to the shared loader. Here the class list is an enum and
//! the dispatch is written once, so a new class cannot be added without the
//! compiler pointing at every place that has to learn about it.

use std::path::Path;

use ironpad_app::db::Db;
use ironpad_common::IronpadNotebook;

use crate::state::AppState;

/// Which storage class a notebook lives in, mirroring the canonical route
/// prefixes (PRD-0048).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Public,
    Shared,
    Mutable,
}

impl Class {
    /// Every class, in route order. `parse` and oEmbed's URL matcher both walk
    /// this, so neither keeps a list of its own.
    pub const ALL: [Self; 3] = [Self::Public, Self::Shared, Self::Mutable];

    /// The path segment naming this class: `/{segment}/{id}`,
    /// `/og/{segment}/{id}.png` and `/embed/{segment}/{id}`.
    #[must_use]
    pub fn segment(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Shared => "shared",
            Self::Mutable => "mutable",
        }
    }

    /// Parses a `{class}` path segment.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.segment() == s)
    }

    /// Reader-facing label printed on the card.
    ///
    /// `Mutable` reads as "shared" deliberately: "mutable share" is ironpad's
    /// internal vocabulary, and someone seeing the card in a feed only needs
    /// to know it is somebody's notebook rather than a bundled one.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "public notebook",
            Self::Shared | Self::Mutable => "shared notebook",
        }
    }

    /// Loads the notebook `id` names in this class, or `None` when there is
    /// nothing a reader may see there.
    ///
    /// `None` covers every refusal alike: a name that never existed, a load
    /// error, and, for `Mutable`, a notebook that is unpublished or private.
    /// The mutable arm is the reader resolve (published copy only, drafts
    /// never; PRD-0057), and callers answer all of these with the same 404 so
    /// a crawler cannot tell a private notebook from a missing one.
    pub async fn load(self, state: &AppState, db: &Db, id: &str) -> Option<IronpadNotebook> {
        match self {
            Self::Public => {
                let site_root = Path::new(state.leptos_options.site_root.as_ref());
                ironpad_app::server_fns::get_public_notebook_core(site_root, id)
                    .await
                    .ok()
            }
            Self::Shared => {
                ironpad_app::server_fns::get_shared_notebook_core(&state.config.data_dir, id)
                    .await
                    .ok()
            }
            Self::Mutable => ironpad_app::server_fns::get_mutable_notebook_core(db, id)
                .await
                .ok()
                .flatten(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Class;

    #[test]
    fn every_class_round_trips_through_its_segment() {
        assert!(Class::ALL
            .iter()
            .all(|c| Class::parse(c.segment()) == Some(*c)));
    }

    #[test]
    fn parse_accepts_only_the_canonical_prefixes() {
        assert_eq!(Class::parse("public"), Some(Class::Public));
        assert_eq!(Class::parse("shared"), Some(Class::Shared));
        assert_eq!(Class::parse("mutable"), Some(Class::Mutable));
        assert_eq!(Class::parse("local"), None);
        assert_eq!(Class::parse(".."), None);
        assert_eq!(Class::parse(""), None);
    }
}
