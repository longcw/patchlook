//! Lays out the history graph: which lane each commit sits in and the lines
//! that join it to its children above and its parents below.

/// A line through one row between two lane columns. A top segment runs from
/// the row's top edge to the commit; a bottom one from the commit to the bottom edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub from: usize,
    pub to: usize,
    pub color: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    pub column: usize,
    pub color: usize,
    pub top: Vec<Segment>,
    pub bottom: Vec<Segment>,
    /// Lane columns this row touches.
    pub width: usize,
}

/// Lane state carried from one row, and one page of history, to the next.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    /// The commit each lane waits for, with the lane's color.
    lanes: Vec<Option<(String, usize)>>,
    next_color: usize,
}

impl Graph {
    fn open_lane(&mut self, id: &str, avoid: usize) -> (usize, usize) {
        let color = self.next_color;
        self.next_color += 1;
        let slot = (0..self.lanes.len())
            .find(|&index| index != avoid && self.lanes[index].is_none())
            .unwrap_or_else(|| {
                self.lanes.push(None);
                self.lanes.len() - 1
            });
        self.lanes[slot] = Some((id.to_owned(), color));
        (slot, color)
    }

    fn waiting_for(&self, id: &str) -> Option<usize> {
        self.lanes
            .iter()
            .position(|lane| lane.as_ref().is_some_and(|(waiting, _)| waiting == id))
    }

    /// Starts a lane for a commit before it is laid out, as the working tree does for HEAD.
    pub fn expect(&mut self, id: &str) -> GraphRow {
        let (column, color) = self.open_lane(id, usize::MAX);
        GraphRow {
            column,
            color,
            top: Vec::new(),
            bottom: vec![Segment {
                from: column,
                to: column,
                color,
            }],
            width: self.lanes.len(),
        }
    }

    pub fn row(&mut self, id: &str, parents: &[String]) -> GraphRow {
        let before = self.lanes.len();
        let mut top = Vec::new();
        let mut converging = Vec::new();
        for (index, lane) in self.lanes.iter().enumerate() {
            if let Some((waiting, color)) = lane
                && waiting == id
            {
                converging.push((index, *color));
            }
        }
        let (column, color) = match converging.first() {
            Some(&(column, color)) => (column, color),
            None => self.open_lane(id, usize::MAX),
        };
        for (index, lane) in self.lanes.iter().enumerate() {
            if let Some((waiting, lane_color)) = lane {
                let to = if waiting == id { column } else { index };
                top.push(Segment {
                    from: index,
                    to,
                    color: *lane_color,
                });
            }
        }
        // A new lane has nothing above it.
        if converging.is_empty() {
            top.retain(|segment| segment.from != column);
        }
        for (index, _) in &converging {
            self.lanes[*index] = None;
        }
        self.lanes[column] = None;

        let mut bottom = Vec::new();
        let mut fresh = Vec::new();
        // The first parent continues straight down; lanes that reach the same
        // commit meet at its row.
        if let Some(first) = parents.first() {
            self.lanes[column] = Some((first.clone(), color));
            fresh.push(column);
            bottom.push(Segment {
                from: column,
                to: column,
                color,
            });
        }
        for parent in parents.iter().skip(1) {
            match self.waiting_for(parent) {
                // Another lane already leads to this merged parent; join it.
                Some(lane) => {
                    let lane_color = self.lanes[lane].as_ref().unwrap().1;
                    bottom.push(Segment {
                        from: column,
                        to: lane,
                        color: lane_color,
                    });
                }
                None => {
                    let (lane, lane_color) = self.open_lane(parent, column);
                    fresh.push(lane);
                    bottom.push(Segment {
                        from: column,
                        to: lane,
                        color: lane_color,
                    });
                }
            }
        }
        for (index, lane) in self.lanes.iter().enumerate() {
            if let Some((_, lane_color)) = lane
                && !fresh.contains(&index)
            {
                bottom.push(Segment {
                    from: index,
                    to: index,
                    color: *lane_color,
                });
            }
        }
        let width = before.max(self.lanes.len()).max(column + 1);
        while self.lanes.last().is_some_and(Option::is_none) {
            self.lanes.pop();
        }
        GraphRow {
            column,
            color,
            top,
            bottom,
            width,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(commits: &[(&str, &[&str])]) -> Vec<GraphRow> {
        let mut graph = Graph::default();
        commits
            .iter()
            .map(|(id, parents)| {
                let parents: Vec<String> = parents.iter().map(|p| p.to_string()).collect();
                graph.row(id, &parents)
            })
            .collect()
    }

    fn lines(segments: &[Segment]) -> Vec<(usize, usize)> {
        segments.iter().map(|s| (s.from, s.to)).collect()
    }

    #[test]
    fn a_linear_history_stays_in_one_lane() {
        let rows = layout(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]);
        assert!(rows.iter().all(|row| row.column == 0 && row.width == 1));
        assert_eq!(lines(&rows[0].top), []);
        assert_eq!(lines(&rows[1].top), [(0, 0)]);
        assert_eq!(lines(&rows[2].bottom), []);
    }

    #[test]
    fn a_merge_opens_a_lane_that_rejoins_at_the_fork() {
        // m merges side branch s into main; both start from base.
        let rows = layout(&[
            ("m", &["main", "s"]),
            ("s", &["base"]),
            ("main", &["base"]),
            ("base", &[]),
        ]);
        assert_eq!(lines(&rows[0].bottom), [(0, 0), (0, 1)]);
        assert_eq!(rows[1].column, 1);
        assert_eq!(lines(&rows[1].top), [(0, 0), (1, 1)]);
        // s continues to base, which main's lane reaches later.
        assert_eq!(rows[2].column, 0);
        assert_eq!(rows[3].column, 0);
        assert_eq!(lines(&rows[3].top), [(0, 0), (1, 0)]);
        assert_ne!(rows[0].color, rows[1].color);
    }

    #[test]
    fn unrelated_tips_get_their_own_lanes() {
        let mut graph = Graph::default();
        let head = graph.expect("b");
        let row = graph.row("b", &["a".into()]);
        assert_eq!(row.column, head.column);
        assert_eq!(lines(&row.top), [(0, 0)]);
        let other = graph.row("x", &["a".into()]);
        assert_eq!(other.column, 1);
        assert_eq!(lines(&other.bottom), [(1, 1), (0, 0)]);
        // Both lanes wait for a and meet at its row.
        let root = graph.row("a", &[]);
        assert_eq!((root.column, lines(&root.top)), (0, vec![(0, 0), (1, 0)]));
    }
}
