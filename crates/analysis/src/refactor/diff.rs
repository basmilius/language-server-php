//! A line diff between two texts, so that a refactor that was worked out on the whole text of a
//! file reaches the client as the few edits it really is.

/// A changed stretch: `old_start..old_end` of the old text becomes `new_start..new_end` of the new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub old_start: usize,
    pub old_end: usize,
    pub new_start: usize,
    pub new_end: usize,
}

/// The most lines two texts may differ by before the stretch between the first and the last
/// difference is sent as one edit.
const MAX_DIFFERENCES: usize = 2000;

fn lines_of(text: &str) -> Vec<(usize, &str)> {
    let mut at = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let start = at;
            at += line.len();
            (start, line)
        })
        .collect()
}

pub(crate) fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    if old == new {
        return Vec::new();
    }
    let (left, right) = (lines_of(old), lines_of(new));
    let mut head = 0;
    while head < left.len() && head < right.len() && left[head].1 == right[head].1 {
        head += 1;
    }
    let mut tail = 0;
    while tail < left.len() - head
        && tail < right.len() - head
        && left[left.len() - 1 - tail].1 == right[right.len() - 1 - tail].1
    {
        tail += 1;
    }
    let middle_left: Vec<&str> = left[head..left.len() - tail].iter().map(|line| line.1).collect();
    let middle_right: Vec<&str> = right[head..right.len() - tail].iter().map(|line| line.1).collect();
    let matches = match_lines(&middle_left, &middle_right);
    let offset_of =
        |lines: &[(usize, &str)], text: &str, index: usize| lines.get(index).map_or(text.len(), |line| line.0);
    let mut out = Vec::new();
    let (mut at_left, mut at_right) = (0, 0);
    let mut push = |from_left: usize, to_left: usize, from_right: usize, to_right: usize| {
        if from_left == to_left && from_right == to_right {
            return;
        }
        let hunk = Hunk {
            old_start: offset_of(&left, old, head + from_left),
            old_end: offset_of(&left, old, head + to_left),
            new_start: offset_of(&right, new, head + from_right),
            new_end: offset_of(&right, new, head + to_right),
        };
        out.push(trim(old, new, hunk));
    };
    for (matched_left, matched_right) in matches {
        push(at_left, matched_left, at_right, matched_right);
        at_left = matched_left + 1;
        at_right = matched_right + 1;
    }
    push(at_left, middle_left.len(), at_right, middle_right.len());
    out.retain(|hunk| hunk.old_start != hunk.old_end || hunk.new_start != hunk.new_end);
    out
}

/// Narrows a stretch of whole lines to the characters that differ.
fn trim(old: &str, new: &str, hunk: Hunk) -> Hunk {
    let (left, right) = (&old[hunk.old_start..hunk.old_end], &new[hunk.new_start..hunk.new_end]);
    let mut head = 0;
    for ((at, a), b) in left.char_indices().zip(right.chars()) {
        if a != b {
            break;
        }
        head = at + a.len_utf8();
    }
    let (left_rest, right_rest) = (&left[head..], &right[head..]);
    let mut tail = 0;
    for (a, b) in left_rest.chars().rev().zip(right_rest.chars().rev()) {
        if a != b {
            break;
        }
        tail += a.len_utf8();
    }
    Hunk {
        old_start: hunk.old_start + head,
        old_end: hunk.old_end - tail,
        new_start: hunk.new_start + head,
        new_end: hunk.new_end - tail,
    }
}

/// The pairs of equal lines of a longest common subsequence, by Myers' algorithm. Past too many
/// differences nothing is matched, which makes the whole stretch one hunk.
fn match_lines(left: &[&str], right: &[&str]) -> Vec<(usize, usize)> {
    let (n, m) = (left.len() as isize, right.len() as isize);
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let max = (n + m).min(MAX_DIFFERENCES as isize);
    let width = (2 * max + 3) as usize;
    let at = |k: isize| (k + max + 1) as usize;
    let mut v = vec![0isize; width];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut reached = None;
    'search: for d in 0..=max {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            let mut y = x - k;
            while x < n && y < m && left[x as usize] == right[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                reached = Some(d);
                break 'search;
            }
            k += 2;
        }
    }
    let Some(last) = reached else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (0..=last).rev() {
        let previous = &trace[d as usize];
        let k = x - y;
        let go_down = k == -d || (k != d && previous[at(k - 1)] < previous[at(k + 1)]);
        let previous_k = if go_down { k + 1 } else { k - 1 };
        let previous_x = previous[at(previous_k)];
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        if d > 0 {
            x = previous_x;
            y = previous_y;
        }
    }
    pairs.reverse();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, new: &str) -> String {
        let mut out = old.to_string();
        for hunk in hunks(old, new).iter().rev() {
            out.replace_range(hunk.old_start..hunk.old_end, &new[hunk.new_start..hunk.new_end]);
        }
        out
    }

    #[test]
    fn hunks_rebuild_the_new_text() {
        let cases = [
            ("a\nb\nc\n", "a\nb\nc\n"),
            ("a\nb\nc\n", "a\nx\nc\n"),
            ("a\nb\nc\nd\ne\n", "a\nc\nd\nz\ne\n"),
            ("", "a\n"),
            ("a\n", ""),
            ("one\ntwo\n", "one\ntwo"),
            ("é\nb\n", "é\nbé\n"),
        ];
        for (old, new) in cases {
            assert_eq!(apply(old, new), new, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn separate_changes_stay_separate() {
        let old = "a\nb\nc\nd\ne\nf\n";
        let new = "a\nB\nc\nd\ne\nF\n";
        assert_eq!(hunks(old, new).len(), 2);
    }

    #[test]
    fn a_hunk_is_narrowed_to_the_characters_that_differ() {
        let found = hunks("foo(1, 2);\n", "foo(1, 3);\n");
        assert_eq!(found.len(), 1);
        assert_eq!(&"foo(1, 3);\n"[found[0].new_start..found[0].new_end], "3");
    }
}
