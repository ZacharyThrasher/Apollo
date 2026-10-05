//! Arranging the rows of a playlist: where a set of picked rows goes (one place up or down, the
//! top, the bottom), the single "move this row before that one" writes that get the service
//! there, and which rows are repeats of an earlier one.
//!
//! The service only knows one edit: move a row in front of another (or to the end). So an
//! arrangement is planned in two steps: [`nudged`] / [`to_edge`] say what order the playlist
//! should have, and [`steps`] turns the difference between two orders into as few single moves
//! as it can, each with the order the playlist holds once it has landed (what an unknown outcome
//! is settled against).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// A playlist's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// One write: `token` goes in front of `before` (`None`: to the end), after which the playlist
/// holds `after`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveStep {
    pub token: String,
    pub before: Option<String>,
    pub after: Vec<String>,
}

/// The order (each entry is a row's index in the list as it was) after the rows `picked` each
/// move one place up (`delta` < 0) or down (`delta` > 0): the rows around them step over them.
/// `None` when nothing can move: no pick, a pick outside the list, or a pick already at the end it
/// moves towards (the whole selection stays put, so a spread-out selection keeps its shape).
pub fn nudged(len: usize, picked: &[usize], delta: isize) -> Option<Vec<usize>> {
    let picked = cleaned(len, picked)?;
    let mut order: Vec<usize> = (0..len).collect();
    if delta < 0 {
        if picked[0] == 0 {
            return None;
        }
        for &p in &picked {
            order.swap(p - 1, p);
        }
    } else if delta > 0 {
        if picked[picked.len() - 1] + 1 == len {
            return None;
        }
        for &p in picked.iter().rev() {
            order.swap(p, p + 1);
        }
    } else {
        return None;
    }
    Some(order)
}

/// The order after the rows `picked` all go to the top or the bottom, keeping their order among
/// themselves. `None` when they are there already or nothing is picked.
pub fn to_edge(len: usize, picked: &[usize], edge: Edge) -> Option<Vec<usize>> {
    let picked = cleaned(len, picked)?;
    let rest = (0..len).filter(|i| picked.binary_search(i).is_err());
    let order: Vec<usize> = match edge {
        Edge::Top => picked.iter().copied().chain(rest).collect(),
        Edge::Bottom => rest.chain(picked.iter().copied()).collect(),
    };
    (order.iter().enumerate().any(|(at, &i)| at != i)).then_some(order)
}

/// The picks sorted and without repeats; `None` when there are none or one is not a row.
fn cleaned(len: usize, picked: &[usize]) -> Option<Vec<usize>> {
    let mut picked = picked.to_vec();
    picked.sort_unstable();
    picked.dedup();
    if picked.is_empty() || picked.last().is_some_and(|&p| p >= len) {
        return None;
    }
    Some(picked)
}

/// The single moves that turn `current` into `desired` (the same tokens in another order), as few
/// as it takes: the longest run of rows already in their relative order stays where it is and
/// every other row is moved in front of the row that follows it in `desired`. Empty when the
/// orders are the same, and when they are not the same set of tokens.
pub fn steps(current: &[String], desired: &[String]) -> Vec<MoveStep> {
    if current.len() != desired.len() || current == desired {
        return Vec::new();
    }
    let place: HashMap<&str, usize> = current
        .iter()
        .enumerate()
        .map(|(at, t)| (t.as_str(), at))
        .collect();
    let Some(wanted) = desired
        .iter()
        .map(|t| place.get(t.as_str()).copied())
        .collect::<Option<Vec<usize>>>()
    else {
        return Vec::new();
    };
    let stays: HashSet<usize> = longest_rising(&wanted).into_iter().collect();
    let mut working = current.to_vec();
    let mut out = Vec::new();
    for at in (0..desired.len()).rev() {
        if stays.contains(&at) {
            continue;
        }
        let token = desired[at].clone();
        let before = desired.get(at + 1).cloned();
        if let Some(from) = working.iter().position(|t| *t == token) {
            working.remove(from);
        }
        let to = before
            .as_ref()
            .and_then(|b| working.iter().position(|t| t == b))
            .unwrap_or(working.len());
        working.insert(to, token.clone());
        out.push(MoveStep {
            token,
            before,
            after: working.clone(),
        });
    }
    out
}

/// Which entries of `seq` (all different) make up a longest strictly rising subsequence: their
/// indices into `seq`, in order.
fn longest_rising(seq: &[usize]) -> Vec<usize> {
    // `tails[k]` is the index of the smallest last entry of a rising run of length k + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut back: Vec<Option<usize>> = vec![None; seq.len()];
    for (i, &value) in seq.iter().enumerate() {
        let k = tails.partition_point(|&t| seq[t] < value);
        back[i] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(i);
        } else {
            tails[k] = i;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        run.push(i);
        at = back[i];
    }
    run.reverse();
    run
}

/// The rows that repeat an earlier one: `(index, index of the first row it repeats)`, in list
/// order. The first occurrence of each key is the one that stays.
pub fn repeats<K: Eq + Hash>(keys: impl IntoIterator<Item = K>) -> Vec<(usize, usize)> {
    let mut first: HashMap<K, usize> = HashMap::new();
    let mut out = Vec::new();
    for (at, key) in keys.into_iter().enumerate() {
        match first.entry(key) {
            std::collections::hash_map::Entry::Occupied(e) => out.push((at, *e.get())),
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(at);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(s: &str) -> Vec<String> {
        s.chars().map(String::from).collect()
    }

    fn apply(order: &[usize], items: &[String]) -> Vec<String> {
        order.iter().map(|&i| items[i].clone()).collect()
    }

    /// What the service does with each step, played back from `current`.
    fn replay(current: &[String], steps: &[MoveStep]) -> Vec<String> {
        let mut list = current.to_vec();
        for step in steps {
            let from = list.iter().position(|t| *t == step.token).expect("held");
            list.remove(from);
            let to = step
                .before
                .as_ref()
                .map(|b| list.iter().position(|t| t == b).expect("target held"))
                .unwrap_or(list.len());
            list.insert(to, step.token.clone());
            assert_eq!(list, step.after, "each step names the order it leaves");
        }
        list
    }

    #[test]
    fn a_single_row_steps_over_its_neighbour() {
        assert_eq!(nudged(4, &[2], -1), Some(vec![0, 2, 1, 3]));
        assert_eq!(nudged(4, &[1], 1), Some(vec![0, 2, 1, 3]));
    }

    #[test]
    fn the_ends_stay_put() {
        assert_eq!(nudged(4, &[0], -1), None);
        assert_eq!(nudged(4, &[3], 1), None);
        assert_eq!(nudged(4, &[], 1), None);
        assert_eq!(nudged(4, &[4], 1), None, "no such row");
        assert_eq!(nudged(4, &[1], 0), None);
    }

    #[test]
    fn a_block_moves_together_and_a_spread_selection_keeps_its_shape() {
        assert_eq!(nudged(5, &[1, 2], -1), Some(vec![1, 2, 0, 3, 4]));
        assert_eq!(nudged(5, &[1, 2], 1), Some(vec![0, 3, 1, 2, 4]));
        assert_eq!(nudged(5, &[1, 3], -1), Some(vec![1, 0, 3, 2, 4]));
        // One pick at the top holds the whole selection.
        assert_eq!(nudged(5, &[0, 3], -1), None);
        assert_eq!(nudged(5, &[1, 4], 1), None);
        // Order and repeats of the picks do not matter.
        assert_eq!(nudged(5, &[2, 1, 1], -1), nudged(5, &[1, 2], -1));
    }

    #[test]
    fn to_the_top_or_bottom_keeps_the_picks_in_their_order() {
        assert_eq!(to_edge(5, &[3], Edge::Top), Some(vec![3, 0, 1, 2, 4]));
        assert_eq!(to_edge(5, &[1, 3], Edge::Top), Some(vec![1, 3, 0, 2, 4]));
        assert_eq!(to_edge(5, &[1, 3], Edge::Bottom), Some(vec![0, 2, 4, 1, 3]));
        assert_eq!(to_edge(5, &[0], Edge::Top), None, "already there");
        assert_eq!(to_edge(5, &[0, 1], Edge::Top), None, "already there");
        assert_eq!(to_edge(5, &[4], Edge::Bottom), None);
        assert_eq!(to_edge(5, &[], Edge::Bottom), None);
        assert_eq!(to_edge(5, &[7], Edge::Bottom), None);
    }

    #[test]
    fn one_row_to_an_end_is_one_write() {
        let cur = tokens("abcde");
        let top = steps(&cur, &apply(&to_edge(5, &[3], Edge::Top).unwrap(), &cur));
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].token, "d");
        assert_eq!(top[0].before.as_deref(), Some("a"));
        let bottom = steps(&cur, &apply(&to_edge(5, &[0], Edge::Bottom).unwrap(), &cur));
        assert_eq!(bottom.len(), 1);
        assert_eq!(bottom[0].before, None, "to the end");
        let nudge = steps(&cur, &apply(&nudged(5, &[2], -1).unwrap(), &cur));
        assert_eq!(nudge.len(), 1);
    }

    #[test]
    fn a_block_moving_one_place_is_one_write() {
        let cur = tokens("abcde");
        let order = nudged(5, &[1, 2], 1).unwrap();
        assert_eq!(steps(&cur, &apply(&order, &cur)).len(), 1);
    }

    #[test]
    fn nothing_to_do_is_no_steps() {
        let cur = tokens("abc");
        assert!(steps(&cur, &cur).is_empty());
        assert!(steps(&cur, &tokens("ab")).is_empty(), "not the same rows");
        assert!(steps(&cur, &tokens("abx")).is_empty(), "not the same rows");
    }

    #[test]
    fn every_arrangement_is_reached_by_its_steps() {
        // Every permutation of six rows, and every pick from them, replays to the wanted order.
        let cur = tokens("abcdef");
        let mut order: Vec<usize> = (0..6).collect();
        let mut seen = 0;
        permute(&mut order, 0, &mut |p| {
            let desired = apply(p, &cur);
            let planned = steps(&cur, &desired);
            assert_eq!(replay(&cur, &planned), desired, "{p:?}");
            assert!(planned.len() <= 5, "never more writes than rows - 1: {p:?}");
            seen += 1;
        });
        assert_eq!(seen, 720);
    }

    fn permute(items: &mut Vec<usize>, k: usize, f: &mut impl FnMut(&[usize])) {
        if k == items.len() {
            f(items);
            return;
        }
        for i in k..items.len() {
            items.swap(k, i);
            permute(items, k + 1, f);
            items.swap(k, i);
        }
    }

    #[test]
    fn picks_to_an_edge_and_nudges_replay_correctly_in_a_long_list() {
        let cur: Vec<String> = (0..40).map(|n| format!("t{n}")).collect();
        for picked in [vec![3, 9, 10, 30], vec![0, 39], vec![5], vec![1, 2, 3, 4]] {
            for edge in [Edge::Top, Edge::Bottom] {
                if let Some(order) = to_edge(40, &picked, edge) {
                    let desired = apply(&order, &cur);
                    let planned = steps(&cur, &desired);
                    assert_eq!(replay(&cur, &planned), desired);
                    assert!(planned.len() <= picked.len(), "{picked:?} {edge:?}");
                }
            }
            for delta in [-1, 1] {
                if let Some(order) = nudged(40, &picked, delta) {
                    let desired = apply(&order, &cur);
                    assert_eq!(replay(&cur, &steps(&cur, &desired)), desired);
                }
            }
        }
    }

    #[test]
    fn repeats_keep_the_first_and_name_it() {
        assert_eq!(repeats(["a", "b", "a", "c", "b", "a"]), vec![(2, 0), (4, 1), (5, 0)]);
        assert!(repeats(["a", "b", "c"]).is_empty());
        assert!(repeats(Vec::<&str>::new()).is_empty());
    }
}
