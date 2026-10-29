// SPDX-License-Identifier: GPL-3.0-or-later
//! Identifiers for the things inside a project.
//!
//! Ids are small integers handed out by the project in order, never reused, and
//! saved in the project file. They stay the same through undo, redo, save and
//! load, so a script or a UI can hold on to a [`ClipId`] across edits.

use std::fmt;

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident, $label:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($label, "{}"), self.0)
            }
        }
    };
}

id_type!(
    /// A media file known to the project.
    AssetId, "asset"
);
id_type!(
    /// One clip on a track.
    ClipId, "clip"
);
id_type!(
    /// One video or audio track.
    TrackId, "track"
);
id_type!(
    /// One sequence (timeline) in the project.
    SequenceId, "sequence"
);
id_type!(
    /// A group of clips that move and trim together, such as a picture and its sound.
    LinkId, "link"
);
id_type!(
    /// A marker on a sequence.
    MarkerId, "marker"
);
