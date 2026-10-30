//! Finds a patch's changed blocks and places them among the rows the diff shows,
//! for the overview ruler beside it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Added,
    Deleted,
    Modified,
}

/// A run of changed lines and the line to scroll to for it: on the new side,
/// or the old side for a pure deletion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    pub line: usize,
    pub original: bool,
    deleted: usize,
    added: usize,
}

/// The file in order: unchanged stretches and change blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Segment {
    Same(usize),
    Change(usize),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overview {
    pub marks: Vec<Mark>,
    segments: Vec<Segment>,
}

/// How the diff lays rows out, which decides where each mark sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Side by side pairs a removed line with an added one on a single row.
    pub split: bool,
    /// Unchanged lines kept around each change; None shows the whole file.
    pub context: Option<usize>,
    /// Unchanged stretches shorter than this stay visible instead of folding.
    pub min_collapsed: usize,
}

/// Marks placed among display rows: (first row, row count) per mark.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub rows: usize,
    pub marks: Vec<(usize, usize)>,
}

impl Placement {
    /// The mark closest to a row, for a click on the ruler.
    pub fn nearest(&self, row: usize) -> Option<usize> {
        (0..self.marks.len()).min_by_key(|&index| {
            let (start, rows) = self.marks[index];
            let end = start + rows.max(1) - 1;
            if row < start {
                start - row
            } else {
                row.saturating_sub(end)
            }
        })
    }
}

impl Overview {
    /// Mirrors the diff's folding: changes keep `context` unchanged lines on each
    /// side, and each longer unchanged stretch shows as a single fold row.
    pub fn place(&self, layout: Layout) -> Placement {
        // One entry per row pair: the change it belongs to and its inline rows.
        let mut pairs: Vec<(Option<usize>, usize)> = Vec::new();
        for segment in &self.segments {
            match *segment {
                Segment::Same(lines) => pairs.extend((0..lines).map(|_| (None, 1))),
                Segment::Change(index) => {
                    let mark = &self.marks[index];
                    for pair in 0..mark.deleted.max(mark.added) {
                        let inline =
                            usize::from(pair < mark.deleted) + usize::from(pair < mark.added);
                        pairs.push((Some(index), inline));
                    }
                }
            }
        }
        let mut visible = vec![layout.context.is_none(); pairs.len()];
        if let Some(context) = layout.context {
            for (index, (change, _)) in pairs.iter().enumerate() {
                if change.is_some() {
                    let start = index.saturating_sub(context);
                    let end = (index + context + 1).min(pairs.len());
                    visible[start..end].fill(true);
                }
            }
            let mut index = 0;
            while index < pairs.len() {
                let start = index;
                while index < pairs.len() && !visible[index] {
                    index += 1;
                }
                if index - start < layout.min_collapsed {
                    visible[start..index].fill(true);
                }
                index += 1;
            }
        }
        let mut placement = Placement {
            rows: 0,
            marks: vec![(0, 0); self.marks.len()],
        };
        let mut folded = false;
        for (index, &(change, inline)) in pairs.iter().enumerate() {
            if !visible[index] {
                if !folded {
                    placement.rows += 1;
                    folded = true;
                }
                continue;
            }
            folded = false;
            let rows = if layout.split { 1 } else { inline };
            if let Some(change) = change {
                let mark = &mut placement.marks[change];
                if mark.1 == 0 {
                    mark.0 = placement.rows;
                }
                mark.1 += rows;
            }
            placement.rows += rows;
        }
        placement
    }
}

/// Reads a single-file unified patch, which with full context covers the whole file.
pub fn overview(patch: &str) -> Overview {
    let mut overview = Overview::default();
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    let mut current: Option<Mark> = None;
    let close = |current: &mut Option<Mark>, overview: &mut Overview| {
        if let Some(mut mark) = current.take() {
            mark.kind = match (mark.added > 0, mark.deleted > 0) {
                (true, true) => MarkKind::Modified,
                (true, false) => MarkKind::Added,
                _ => MarkKind::Deleted,
            };
            overview
                .segments
                .push(Segment::Change(overview.marks.len()));
            overview.marks.push(mark);
        }
    };
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            close(&mut current, &mut overview);
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
        match line.as_bytes().first() {
            Some(byte @ (b'+' | b'-')) => {
                let added = *byte == b'+';
                let mark = current.get_or_insert(Mark {
                    kind: MarkKind::Modified,
                    line: if added { new } else { old },
                    original: !added,
                    deleted: 0,
                    added: 0,
                });
                if added {
                    // A block that adds lines is reached on the new side.
                    if mark.added == 0 && mark.original {
                        mark.line = new;
                        mark.original = false;
                    }
                    mark.added += 1;
                    new += 1;
                } else {
                    mark.deleted += 1;
                    old += 1;
                }
            }
            Some(b' ') | None => {
                close(&mut current, &mut overview);
                match overview.segments.last_mut() {
                    Some(Segment::Same(lines)) => *lines += 1,
                    _ => overview.segments.push(Segment::Same(1)),
                }
                old += 1;
                new += 1;
            }
            // "\ No newline at end of file" is not a row.
            _ => {}
        }
    }
    close(&mut current, &mut overview);
    overview
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "--- a/f\n+++ b/f\n@@ -1,13 +1,13 @@\n a\n-b\n+B\n+B2\n c\n d\n e\n f\n g\n h\n i\n j\n-gone\n k\n\\ No newline at end of file\n";

    #[test]
    fn blocks_are_classified_with_the_line_to_open() {
        let marks: Vec<_> = overview(PATCH)
            .marks
            .iter()
            .map(|mark| (mark.kind, mark.line, mark.original))
            .collect();
        assert_eq!(
            marks,
            [
                (MarkKind::Modified, 2, false),
                (MarkKind::Deleted, 11, true)
            ]
        );
    }

    #[test]
    fn marks_follow_the_layout_and_folding_of_the_diff() {
        let overview = overview(PATCH);
        let full = |split| Layout {
            split,
            context: None,
            min_collapsed: 2,
        };
        // Inline lists removed and added lines apart; split pairs them on one row.
        let inline = overview.place(full(false));
        assert_eq!(
            (inline.rows, inline.marks.clone()),
            (14, vec![(1, 3), (12, 1)])
        );
        let split = overview.place(full(true));
        assert_eq!(
            (split.rows, split.marks.clone()),
            (13, vec![(1, 2), (11, 1)])
        );
        // One context line each side: d..i fold into one row between the changes.
        let folded = overview.place(Layout {
            split: true,
            context: Some(1),
            min_collapsed: 2,
        });
        assert_eq!(
            (folded.rows, folded.marks.clone()),
            (8, vec![(1, 2), (6, 1)])
        );
        assert_eq!(folded.nearest(7), Some(1));
        assert_eq!(folded.nearest(0), Some(0));
    }
}
