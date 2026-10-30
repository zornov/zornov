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

use super::intra::code_block_rd;
use super::rdcost::lambda;
use crate::recon::FrameBuf;
use crate::types::{BlockDecision, Frame};

/// Bits charged for a partition symbol when comparing a block with its split.
const PARTITION_BITS: f64 = 2.0;

/// Searches the partition of the `n4` x `n4` node at `(r, c)` of an intra frame: the block whole or
/// split into four, recursively, whichever costs less (squared error plus lambda times the
/// estimated bits). The blocks are coded into `recon` as they are decided, so the winner is left
/// reconstructed there; `blocks` receives them in decode order. Returns the cost.
///
/// A node that reaches past the frame edge follows [`fixed_partition`]'s forced splits, with the
/// search running inside the pieces. Blocks go down to `min_n4` (in 4x4 units, at least 2: 8x8).
#[allow(clippy::too_many_arguments)]
pub fn search_partition(src: &Frame, recon: &mut FrameBuf, r: usize, c: usize, n4: usize, mi_rows: usize, mi_cols: usize, search_modes: bool, min_n4: usize, blocks: &mut Vec<BlockDecision>) -> f64 {
    if r >= mi_rows || c >= mi_cols {
        return 0.0;
    }
    let half = n4 / 2;
    let whole_allowed = n4 == 1 || (r + half < mi_rows && c + half < mi_cols);
    let lam = lambda(recon.quant.base_q_idx, recon.bit_depth);
    if !whole_allowed {
        // forced: the pieces fixed_partition would make, each searched where it is a full node
        let mut shapes = Vec::new();
        fixed_partition(r, c, n4, mi_rows, mi_cols, &mut shapes);
        let mut total = 0.0;
        if shapes.len() == 1 || shapes.iter().any(|s| s.2 != s.3) {
            // a HORZ / VERT piece: coded as it is
            for shape in shapes {
                let (d, cost) = code_block_rd(src, recon, shape, search_modes);
                blocks.push(d);
                total += cost;
            }
            return total;
        }
        for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
            total += search_partition(src, recon, r + dr, c + dc, half, mi_rows, mi_cols, search_modes, min_n4, blocks);
        }
        return total;
    }
    let base = recon.save_region(r, c, n4);
    let (whole, whole_cost) = code_block_rd(src, recon, (r, c, n4, n4), search_modes);
    let whole_cost = whole_cost + lam * PARTITION_BITS;
    if n4 <= min_n4.max(2) {
        blocks.push(whole);
        return whole_cost;
    }
    let after_whole = recon.save_region(r, c, n4);
    recon.restore_region(&base);
    let mut split = Vec::new();
    let mut split_cost = lam * PARTITION_BITS;
    for (dr, dc) in [(0, 0), (0, half), (half, 0), (half, half)] {
        split_cost += search_partition(src, recon, r + dr, c + dc, half, mi_rows, mi_cols, search_modes, min_n4, &mut split);
    }
    if whole_cost <= split_cost {
        recon.restore_region(&after_whole);
        blocks.push(whole);
        whole_cost
    } else {
        blocks.extend(split);
        split_cost
    }
}
