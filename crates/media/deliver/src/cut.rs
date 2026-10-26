// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Planning a cutdown: which shots of the main lane to keep, drop or
//! shorten so a programme fits 15, 30 or 60 seconds. Pure arithmetic over
//! scored shots; the caller applies the plan with edit operations.

/// One shot of the main lane.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub id: u32,
    pub dur: f64,
    /// Higher is more worth keeping.
    pub score: f64,
    /// Must stay (the opening hook, the closing card, what the person named).
    pub keep: bool,
    /// Must go.
    pub drop: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Keep,
    Drop,
    /// Keep the start of the shot, this long.
    Trim(f64),
}

/// The shortest a shot is allowed to become.
pub const MIN_SHOT: f64 = 0.8;

/// An action per shot, in order, so the kept length is `target` (or as
/// close as the shots allow). The first and last shots are kept unless
/// marked `drop`.
pub fn plan(shots: &[Shot], target: f64) -> Vec<(u32, Action)> {
    let n = shots.len();
    let mut state: Vec<Action> = shots.iter().map(|s| if s.drop { Action::Drop } else { Action::Keep }).collect();
    let protect = |i: usize| shots[i].keep || i == 0 || i + 1 == n;
    let mut total: f64 = shots.iter().zip(&state).filter(|(_, a)| **a != Action::Drop).map(|(s, _)| s.dur).sum();
    // Drop the least valuable shots while the cut still has at least `target`.
    let mut order: Vec<usize> = (0..n).filter(|&i| !protect(i) && state[i] == Action::Keep).collect();
    order.sort_by(|&a, &b| shots[a].score.total_cmp(&shots[b].score).then(shots[b].dur.total_cmp(&shots[a].dur)));
    for i in order {
        if total - shots[i].dur >= target - 1e-9 {
            total -= shots[i].dur;
            state[i] = Action::Drop;
        }
    }
    // Still long: shorten the kept shots, the least valuable first, each down to MIN_SHOT.
    if total > target + 1e-9 {
        let mut order: Vec<usize> = (0..n).filter(|&i| state[i] == Action::Keep).collect();
        order.sort_by(|&a, &b| shots[a].score.total_cmp(&shots[b].score));
        for pass in 0..2 {
            for &i in &order {
                if total <= target + 1e-9 {
                    break;
                }
                let floor = if pass == 0 { MIN_SHOT.max(shots[i].dur * 0.5) } else { MIN_SHOT };
                let current = match state[i] {
                    Action::Trim(d) => d,
                    _ => shots[i].dur,
                };
                let can = (current - floor).max(0.0);
                let cut = can.min(total - target);
                if cut > 1e-9 {
                    state[i] = Action::Trim(current - cut);
                    total -= cut;
                }
            }
        }
    }
    // Still long (every shot at its floor): drop protected-but-not-kept shots from the lowest score.
    if total > target + 1e-9 {
        let mut order: Vec<usize> = (0..n).filter(|&i| state[i] != Action::Drop && !shots[i].keep && (i != 0 && i + 1 != n)).collect();
        order.sort_by(|&a, &b| shots[a].score.total_cmp(&shots[b].score));
        for i in order {
            if total <= target + 1e-9 {
                break;
            }
            let cur = match state[i] {
                Action::Trim(d) => d,
                _ => shots[i].dur,
            };
            total -= cur;
            state[i] = Action::Drop;
        }
    }
    shots.iter().zip(state).map(|(s, a)| (s.id, a)).collect()
}

/// The length a plan keeps.
pub fn kept(shots: &[Shot], plan: &[(u32, Action)]) -> f64 {
    shots.iter().zip(plan).map(|(s, (_, a))| match a {
        Action::Keep => s.dur,
        Action::Drop => 0.0,
        Action::Trim(d) => *d,
    }).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shots(durs: &[f64], scores: &[f64]) -> Vec<Shot> {
        durs.iter().zip(scores).enumerate().map(|(i, (d, s))| Shot { id: i as u32 + 1, dur: *d, score: *s, keep: false, drop: false }).collect()
    }

    #[test]
    fn low_value_shots_go_first_and_the_ends_stay() {
        let s = shots(&[5.0, 5.0, 5.0, 5.0, 5.0, 5.0], &[1.0, 0.2, 3.0, 0.1, 2.0, 1.0]);
        let p = plan(&s, 20.0);
        assert_eq!(p[3].1, Action::Drop);
        assert_eq!(p[0].1, Action::Keep);
        assert_eq!(p[5].1, Action::Keep);
        assert!((kept(&s, &p) - 20.0).abs() < 1e-6, "{}", kept(&s, &p));
        let p = plan(&s, 15.0);
        assert!((kept(&s, &p) - 15.0).abs() < 1e-6);
        assert_eq!((p[1].1.clone(), p[3].1.clone()), (Action::Drop, Action::Drop));
    }

    #[test]
    fn trims_make_up_the_difference_exactly() {
        let s = shots(&[6.0, 7.0, 8.0], &[1.0, 0.5, 1.0]);
        let p = plan(&s, 12.5);
        assert!((kept(&s, &p) - 12.5).abs() < 1e-6, "{p:?}");
        assert!(p.iter().all(|(_, a)| match a { Action::Trim(d) => *d >= MIN_SHOT, _ => true }));
        // Already short enough: nothing changes.
        let p = plan(&s, 60.0);
        assert!(p.iter().all(|(_, a)| *a == Action::Keep));
    }

    #[test]
    fn named_shots_stay_and_unwanted_ones_go() {
        let mut s = shots(&[4.0, 4.0, 4.0, 4.0, 4.0], &[1.0, 0.1, 0.1, 5.0, 1.0]);
        s[1].keep = true;
        s[3].drop = true;
        let p = plan(&s, 12.0);
        assert_eq!(p[1].1, Action::Keep);
        assert_eq!(p[3].1, Action::Drop);
        assert!((kept(&s, &p) - 12.0).abs() < 1e-6, "{}", kept(&s, &p));
    }

    #[test]
    fn impossible_targets_get_as_close_as_the_ends_allow() {
        let s = shots(&[3.0, 3.0, 3.0], &[1.0, 1.0, 1.0]);
        let p = plan(&s, 1.0);
        assert!(kept(&s, &p) >= 2.0 * MIN_SHOT - 1e-9, "{}", kept(&s, &p));
        assert_eq!(p[0].1 == Action::Drop || p[2].1 == Action::Drop, false);
    }
}
