//! Did-you-mean: the declared name closest to a misspelt one.

/// Most edits between a name and the one suggested for it.
const MAX_EDITS: usize = 3;

/// A name allows one edit per this many characters, up to [`MAX_EDITS`].
const CHARACTERS_PER_EDIT: usize = 3;

/// Most edits temple allows between a name and the one it suggests.
pub(super) const TEMPLE_EDITS: usize = 2;

/// Most bytes a character takes in UTF-8.
const MAX_CHARACTER_BYTES: usize = 4;

/// Most edits [`closest`] allows for a name of `target` characters.
fn edits_for(target: usize) -> usize {
    (target / CHARACTERS_PER_EDIT).clamp(1, MAX_EDITS)
}

/// The most work, in table cells, a did-you-mean search allowing `edits`
/// edits does for a name of `target` characters on a candidate of
/// `candidate` bytes: it counts the candidate's characters, and fills a
/// table only when their lengths are within `edits` of each other. A
/// character takes one to four bytes, so this holds for any text.
pub(super) fn cost(target: usize, candidate: usize, edits: usize) -> u64 {
    let compared =
        candidate + edits >= target && candidate.div_ceil(MAX_CHARACTER_BYTES) <= target + edits;
    let table = if compared {
        (target as u64 + 1) * (candidate.min(target + edits) as u64 + 1)
    } else {
        0
    };
    candidate as u64 + table
}

/// The most work [`closest`] does for `target` among `candidates`.
pub(super) fn closest_cost<'a>(target: &str, candidates: impl Iterator<Item = &'a str>) -> u64 {
    let target = target.chars().count();
    let edits = edits_for(target);
    candidates
        .map(|candidate| cost(target, candidate.len(), edits))
        .sum()
}

/// The closest candidate within a few edits: the nearest, then the first by
/// name, so the hint never depends on declaration order.
pub(super) fn closest<'a>(
    target: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let target: Vec<char> = target.chars().collect();
    let limit = edits_for(target.len());
    let mut rows = Vec::new();
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        // a length gap alone costs that many edits
        if candidate.chars().count().abs_diff(target.len()) > limit {
            continue;
        }
        if let Some(distance) = edit_distance_within(&target, candidate, limit, &mut rows) {
            let better = match best {
                None => true,
                Some(best) => (distance, candidate) < best,
            };
            if better {
                best = Some((distance, candidate));
            }
        }
    }
    best.map(|(_, candidate)| candidate)
}

/// Edit distance counting a swap of two neighbouring letters as one edit,
/// so `nxet` is one step from `next`, or `None` once it must exceed `limit`.
/// `rows` is scratch space reused between calls.
fn edit_distance_within(a: &[char], b: &str, limit: usize, rows: &mut Vec<usize>) -> Option<usize> {
    let b: Vec<char> = b.chars().collect();
    let width = b.len() + 1;
    rows.clear();
    rows.resize(3 * width, 0);
    // rows two back, one back, and the current one, rotated each step
    let (mut back2, mut back1, mut current) = (0, width, 2 * width);
    for (j, cell) in rows[back1..back1 + width].iter_mut().enumerate() {
        *cell = j;
    }
    let mut back1_min = 0;
    for i in 1..=a.len() {
        rows[current] = i;
        let mut current_min = i;
        for j in 1..width {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut distance = (rows[back1 + j] + 1)
                .min(rows[current + j - 1] + 1)
                .min(rows[back1 + j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                distance = distance.min(rows[back2 + j - 2] + 1);
            }
            rows[current + j] = distance;
            current_min = current_min.min(distance);
        }
        // a swap reaches back two rows, so stop only when both are past the limit
        if current_min > limit && back1_min > limit {
            return None;
        }
        back1_min = current_min;
        (back2, back1, current) = (back1, current, back2);
    }
    let distance = rows[back1 + width - 1];
    (distance <= limit).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_names_a_few_edits_away() {
        assert_eq!(closest("nxet", ["next", "done"]), Some("next"));
        let long = ["approve_refund_request"];
        assert_eq!(closest("approve_refnd_rquest", long), Some(long[0]));
        // five edits are too many, however long the name
        assert_eq!(closest("approve_rfnd_rqst", long), None);
    }
}
