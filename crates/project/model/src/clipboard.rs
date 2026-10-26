// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Copied clips, ready to paste into any sequence or project.

use crate::clip::Clip;
use crate::edit::EditError;
use crate::id::{ClipId, SequenceId};
use crate::media::MediaItem;
use crate::project::Project;
use crate::sequence::Transition;
use serde::{Deserialize, Serialize};

/// Clips copied from a sequence, with times relative to the earliest one
/// and lanes recorded as positions from the spine, so the content can be
/// pasted anywhere with [`crate::Edit::Paste`]. The media items the clips
/// use travel along, so pasting into another project works too.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Clipboard {
    /// The copied clips; `clip.start` is relative to the earliest clip.
    pub clips: Vec<ClipboardClip>,
    /// Transitions between copied spine clips.
    #[serde(default)]
    pub transitions: Vec<Transition>,
    /// Media items used by the clips.
    #[serde(default)]
    pub media: Vec<MediaItem>,
}

/// One copied clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipboardClip {
    /// Lane position the clip came from (0 spine, + above, − below).
    pub lane_position: i32,
    /// The clip, with `start` relative to the clipboard's start.
    pub clip: Clip,
}

impl Clipboard {
    /// Copies clips `ids` from sequence `seq`.
    pub fn copy(project: &Project, seq: SequenceId, ids: &[ClipId]) -> Result<Clipboard, EditError> {
        let s = project.sequence(seq).ok_or_else(|| EditError::NotFound(seq.to_string()))?;
        let mut clips: Vec<&Clip> = ids.iter().map(|id| s.clip(*id).ok_or_else(|| EditError::NotFound(id.to_string()))).collect::<Result<_, _>>()?;
        clips.sort_by_key(|c| (c.start, c.id));
        let origin = clips.first().map_or(0, |c| c.start);
        let mut out = Clipboard::default();
        for c in &clips {
            let mut copy = (*c).clone();
            copy.start -= origin;
            if let Some(m) = c.media_id().and_then(|m| project.media_item(m))
                && !out.media.iter().any(|x| x.id == m.id)
            {
                out.media.push(m.clone());
            }
            out.clips.push(ClipboardClip { lane_position: s.lane_position(c.lane).unwrap_or(0), clip: copy });
        }
        out.transitions = s.transitions.iter().filter(|t| ids.contains(&t.left) && ids.contains(&t.right)).cloned().collect();
        Ok(out)
    }

    /// Whether nothing was copied.
    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// Serialises the clipboard (e.g. for the system clipboard).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("clipboard serialises")
    }

    /// Reads a clipboard written by [`Clipboard::to_json`].
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
