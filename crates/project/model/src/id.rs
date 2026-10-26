// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Typed identifiers.
//!
//! All ids in a project come from one counter ([`crate::Project::alloc_id`]),
//! so an id is unique across every kind of object, not just within its own
//! kind. Distinct Rust types stop a clip id being passed where a lane id is
//! expected. Ids are plain numbers in the file format.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! ids {
    ($($(#[$doc:meta])* $name:ident),* $(,)?) => {$(
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.0)
            }
        }
    )*};
}

ids! {
    /// Identifies a [`crate::Sequence`].
    SequenceId,
    /// Identifies a [`crate::Lane`].
    LaneId,
    /// Identifies a [`crate::Clip`].
    ClipId,
    /// Identifies a [`crate::Transition`].
    TransitionId,
    /// Identifies a [`crate::Marker`].
    MarkerId,
    /// Identifies a [`crate::MediaItem`].
    MediaId,
    /// Identifies an [`crate::Effect`].
    EffectId,
    /// Identifies a [`crate::Mask`].
    MaskId,
    /// Groups clips that are selected and moved together.
    GroupId,
    /// Links clips that belong together, such as a video clip and its detached audio.
    LinkId,
    /// Identifies a [`crate::Preset`].
    PresetId,
}
