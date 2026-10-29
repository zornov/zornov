// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M6: partition choice. Phase (a) uses a fixed partition: whole 64x64 blocks where the frame
//! allows, and the splits the decoder forces along the right and bottom edges.

/// A block: `(mi_row, mi_col, w4, h4)` in 4x4 units.
pub type Shape = (usize, usize, usize, usize);

/// The blocks of the fixed partition of the `n4` x `n4` node at `(r, c)`, in decode order.
///
/// A node is coded whole when at least half of it lies inside the frame in both directions
/// (`PARTITION_NONE`). With only the top half or left half inside it is cut into one `HORZ` or
/// `VERT` block, and with neither it is split into four.
pub fn fixed_partition(r: usize, c: usize, n4: usize, mi_rows: usize, mi_cols: usize, out: &mut Vec<Shape>) {
    if r >= mi_rows || c >= mi_cols {
        return;
    }
    let half = n4 / 2;
    let has_rows = r + half < mi_rows;
    let has_cols = c + half < mi_cols;
    if n4 == 1 || (has_rows && has_cols) {
        out.push((r, c, n4, n4));
    } else if has_cols {
        out.push((r, c, n4, half));
    } else if has_rows {
        out.push((r, c, half, n4));
    } else {
        for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
            fixed_partition(r + dr, c + dc, half, mi_rows, mi_cols, out);
        }
    }
}
