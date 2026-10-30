//! Finds a patch's changed blocks for the overview ruler beside the diff.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Added,
    Deleted,
    Modified,
}

/// A run of changed lines, located among all rows of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    pub start: usize,
    pub rows: usize,
    /// The line to scroll to: on the new side, or the old side for a pure deletion.
    pub line: usize,
    pub original: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overview {
    /// Every context, added and removed row of the patch, as an inline diff lists them.
    pub rows: usize,
    pub marks: Vec<Mark>,
}

impl Overview {
    /// The mark closest to a row, for a click on the ruler.
    pub fn nearest(&self, row: usize) -> Option<&Mark> {
        self.marks.iter().min_by_key(|mark| {
            let end = mark.start + mark.rows.max(1) - 1;
            if row < mark.start {
                mark.start - row
            } else {
                row.saturating_sub(end)
            }
        })
    }
}

/// Reads a single-file unified patch. With full context, its rows are the whole file.
pub fn overview(patch: &str) -> Overview {
    let mut overview = Overview::default();
    let (mut old, mut new) = (0, 0);
    let mut block: Option<(Mark, bool, bool)> = None;
    let finish = |block: &mut Option<(Mark, bool, bool)>, marks: &mut Vec<Mark>| {
        if let Some((mut mark, added, deleted)) = block.take() {
            mark.kind = match (added, deleted) {
                (true, true) => MarkKind::Modified,
                (true, false) => MarkKind::Added,
                _ => MarkKind::Deleted,
            };
            marks.push(mark);
        }
    };
    let mut in_hunk = false;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            finish(&mut block, &mut overview.marks);
            let mut ranges = header.split_whitespace();
            let start = |range: Option<&str>| {
                range
                    .and_then(|range| range[1..].split(',').next()?.parse::<usize>().ok())
                    .unwrap_or(1)
            };
            old = start(ranges.next());
            new = start(ranges.next());
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        let change = match line.as_bytes().first() {
            Some(b'+') => Some(true),
            Some(b'-') => Some(false),
            Some(b' ') | None => None,
            // "\ No newline at end of file" and anything else is not a row.
            _ => continue,
        };
        match change {
            Some(added) => {
                let (mark, has_added, has_deleted) = block.get_or_insert((
                    Mark {
                        kind: MarkKind::Modified,
                        start: overview.rows,
                        rows: 0,
                        line: if added { new } else { old },
                        original: !added,
                    },
                    false,
                    false,
                ));
                mark.rows += 1;
                if added {
                    // A block that adds lines is reached on the new side.
                    if !*has_added && mark.original {
                        mark.line = new;
                        mark.original = false;
                    }
                    *has_added = true;
                    new += 1;
                } else {
                    *has_deleted = true;
                    old += 1;
                }
            }
            None => {
                finish(&mut block, &mut overview.marks);
                old += 1;
                new += 1;
            }
        }
        overview.rows += 1;
    }
    finish(&mut block, &mut overview.marks);
    overview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_classified_and_located_among_all_rows() {
        let patch = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,6 +1,6 @@\n a\n-b\n+B\n c\n+new\n d\n-gone\n e\n\\ No newline at end of file\n";
        let overview = overview(patch);
        assert_eq!(overview.rows, 8);
        let marks: Vec<_> = overview
            .marks
            .iter()
            .map(|m| (m.kind, m.start, m.rows, m.line, m.original))
            .collect();
        assert_eq!(
            marks,
            [
                (MarkKind::Modified, 1, 2, 2, false),
                (MarkKind::Added, 4, 1, 4, false),
                (MarkKind::Deleted, 6, 1, 5, true),
            ]
        );
        assert_eq!(overview.nearest(0).unwrap().start, 1);
        assert_eq!(overview.nearest(8).unwrap().start, 6);
    }
}
