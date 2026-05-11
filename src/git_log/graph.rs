use crate::git_log::data::{CommitRecord, Sha};

#[derive(Clone, Debug, PartialEq)]
pub struct LaneRow {
    pub sha: Sha,
    pub own_lane: u8,
    /// Which lanes the parents occupy. The first entry is always
    /// `own_lane` for the first parent (linear continuation) — except
    /// for root commits, where this is empty.
    pub parent_lanes: Vec<u8>,
    /// Lanes that existed BEFORE this row's draw and don't continue
    /// past it (closing branches). Used by the painter to draw lane
    /// caps.
    pub terminating_lanes: Vec<u8>,
    /// Lanes that pass STRAIGHT THROUGH this row — active before and
    /// active after, but neither this commit's `own_lane` nor any of
    /// its `parent_lanes`. Each pair is `(lane_index, color_slot)` so
    /// the painter can draw a vertical segment in the lane's
    /// branch-stable color. Without this, a branch tip whose parent
    /// is many rows below disappears between rows belonging to
    /// other branches.
    pub passthrough_lanes: Vec<(u8, u8)>,
    /// Color slot in the 8-color palette. Approximates "color per
    /// branch" — see ColorSeeder for details.
    pub color: u8,
    /// How many lanes are still active after this row.
    pub visible_lanes_after: u8,
}

#[derive(Clone, Debug, Default)]
pub struct LaneFrame {
    pub rows: Vec<LaneRow>,
    pub max_lane: u8,
}

/// Stable color picker keyed on `(lane_index, allocation_epoch)`.
/// Each time a lane is freshly claimed (after being free or never
/// used), the epoch increments. Same (lane, epoch) → same color.
pub struct ColorSeeder {
    epochs: Vec<u32>,           // per-lane allocation count
}

impl ColorSeeder {
    pub fn new() -> Self {
        Self { epochs: Vec::new() }
    }
    /// Call when allocating lane `i` for a new branch. Returns the
    /// color slot (0..8) for that allocation.
    pub fn allocate(&mut self, lane: usize) -> u8 {
        while self.epochs.len() <= lane {
            self.epochs.push(0);
        }
        self.epochs[lane] += 1;
        let h = (lane as u32 * 7919) ^ (self.epochs[lane] * 31337);
        (h % 8) as u8
    }
    /// Color for a row whose lane was allocated in the current epoch.
    /// (Doesn't increment.)
    pub fn current(&self, lane: usize) -> u8 {
        let e = *self.epochs.get(lane).unwrap_or(&1);
        let h = (lane as u32 * 7919) ^ (e * 31337);
        (h % 8) as u8
    }
}

impl Default for ColorSeeder {
    fn default() -> Self { Self::new() }
}

/// Build a LaneFrame from commits in display order (newest first).
/// Algorithm walks oldest→newest internally to track lane ownership,
/// then reverses back to display order.
pub fn layout(commits: &[CommitRecord]) -> LaneFrame {
    if commits.is_empty() {
        return LaneFrame::default();
    }

    // active_lanes[i] = sha that the next commit on column i must be.
    // None = column free.
    let mut active_lanes: Vec<Option<Sha>> = Vec::new();
    let mut seeder = ColorSeeder::new();
    // Walk commits newest → oldest. Input is already in newest-first
    // display order. Each commit either finds its sha already in a
    // lane (claimed by a previously-processed child) or allocates a
    // fresh lane. Then for each parent: first parent claims the same
    // lane (linear continuation); subsequent parents fork off into
    // fresh lanes — those will be picked up when we later reach the
    // parent commit. Output rows are also in newest-first order,
    // matching the input — no reversal needed.
    let mut rows: Vec<LaneRow> = Vec::with_capacity(commits.len());

    for c in commits.iter() {
        // Snapshot lanes BEFORE this row's mutations — used to
        // identify terminating lanes.
        let lanes_before = active_lanes.clone();

        // 1. Find the lane waiting for this commit (or allocate a new one).
        let own_lane = match active_lanes.iter().position(|l| l.as_ref() == Some(&c.sha)) {
            Some(idx) => idx,
            None => {
                // Orphan / fresh tip — leftmost free or push.
                let slot = active_lanes.iter().position(Option::is_none).unwrap_or(active_lanes.len());
                if slot == active_lanes.len() {
                    active_lanes.push(None);
                }
                seeder.allocate(slot);
                slot
            }
        };

        // 2. First parent claims the same lane (linear continuation),
        //    UNLESS that parent is already pending in another lane —
        //    then we terminate our lane and merge into the existing
        //    one. Without this, two branches that share a parent
        //    leave a dangling lane that draws passthrough forever
        //    because position() finds the first match when the parent
        //    is finally processed.
        let mut parent_lanes: Vec<u8> = Vec::new();
        if let Some(p0) = c.parents.first() {
            let already_tracked = active_lanes
                .iter()
                .enumerate()
                .find(|(i, l)| *i != own_lane && l.as_ref() == Some(p0))
                .map(|(i, _)| i);
            if let Some(other) = already_tracked {
                active_lanes[own_lane] = None;
                parent_lanes.push(other as u8);
            } else {
                active_lanes[own_lane] = Some(p0.clone());
                parent_lanes.push(own_lane as u8);
            }
        } else {
            active_lanes[own_lane] = None; // root commit
        }

        // 3. Subsequent parents → branch off into new lanes, OR merge
        //    into an existing lane if that parent is already pending.
        for p in c.parents.iter().skip(1) {
            let already_tracked = active_lanes
                .iter()
                .enumerate()
                .find(|(_, l)| l.as_ref() == Some(p))
                .map(|(i, _)| i);
            if let Some(other) = already_tracked {
                parent_lanes.push(other as u8);
                continue;
            }
            let slot = active_lanes.iter().position(Option::is_none).unwrap_or(active_lanes.len());
            if slot == active_lanes.len() {
                active_lanes.push(None);
            }
            active_lanes[slot] = Some(p.clone());
            seeder.allocate(slot);
            parent_lanes.push(slot as u8);
        }

        // 4. Compact: trailing Nones drop off so visual width stays minimal.
        while matches!(active_lanes.last(), Some(None)) {
            active_lanes.pop();
        }

        // Lanes that existed before but don't exist after = terminating.
        let terminating_lanes: Vec<u8> = lanes_before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let still_alive = i < active_lanes.len() && active_lanes[i].is_some();
                if l.is_some() && !still_alive && i != own_lane {
                    Some(i as u8)
                } else {
                    None
                }
            })
            .collect();

        // Passthrough lanes: alive before AND after this row, and not
        // this commit's own lane. The painter draws a vertical
        // segment spanning the full row height; this carries the lane
        // visually across intermediate commits.
        //
        // Note we do NOT exclude `parent_lanes`. The parent-lane
        // bezier only covers the bottom half of this row (from this
        // commit's dot center down to the next row's center). The TOP
        // half of a lane that was alive coming in still needs the
        // vertical segment — otherwise "merge into existing lane"
        // commits leave a 1-row gap above the curve, and the user
        // sees the branch lane disconnect on the bottom side of the
        // merge. The bezier's lower half and the passthrough segment
        // overlap cleanly in the bottom-left corner of the row.
        let passthrough_lanes: Vec<(u8, u8)> = lanes_before
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let alive_after =
                    i < active_lanes.len() && active_lanes[i].is_some();
                let alive_before = l.is_some();
                if !(alive_before && alive_after) {
                    return None;
                }
                let lane_u8 = i as u8;
                if lane_u8 == own_lane as u8 {
                    return None;
                }
                Some((lane_u8, seeder.current(i)))
            })
            .collect();

        let color = seeder.current(own_lane);

        rows.push(LaneRow {
            sha: c.sha.clone(),
            own_lane: own_lane as u8,
            parent_lanes,
            terminating_lanes,
            passthrough_lanes,
            color,
            visible_lanes_after: active_lanes.len() as u8,
        });
    }

    let max_lane = rows.iter().map(|r| r.visible_lanes_after).max().unwrap_or(1);
    LaneFrame { rows, max_lane }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_log::data::CommitRecord;

    fn cr(sha: &str, parents: &[&str]) -> CommitRecord {
        CommitRecord {
            sha: sha.to_string(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            author: "A".to_string(),
            date: "2026-05-01T10:00:00+00:00".to_string(),
            subject: "S".to_string(),
            refs_decoration: String::new(),
        }
    }

    #[test]
    fn empty_input_returns_empty_frame() {
        let frame = layout(&[]);
        assert!(frame.rows.is_empty());
        assert_eq!(frame.max_lane, 0);
    }

    #[test]
    fn straight_line_no_merges() {
        // c3 -> c2 -> c1 -> root (display: newest-first)
        let commits = vec![
            cr("c3", &["c2"]),
            cr("c2", &["c1"]),
            cr("c1", &["root"]),
            cr("root", &[]),
        ];
        let frame = layout(&commits);
        assert_eq!(frame.rows.len(), 4);
        for r in &frame.rows {
            assert_eq!(r.own_lane, 0, "row {} not on lane 0", r.sha);
        }
    }

    #[test]
    fn fork_and_merge_two_branches() {
        //   m       (merge of c2, b1)
        //   |\
        //   c2 b1
        //   | /
        //   c1
        let commits = vec![
            cr("m",  &["c2", "b1"]),
            cr("c2", &["c1"]),
            cr("b1", &["c1"]),
            cr("c1", &[]),
        ];
        let frame = layout(&commits);

        let m_row = frame.rows.iter().find(|r| r.sha == "m").unwrap();
        assert_eq!(m_row.parent_lanes.len(), 2);
    }

    #[test]
    fn octopus_three_parents() {
        let commits = vec![
            cr("o", &["p1", "p2", "p3"]),
            cr("p1", &[]),
            cr("p2", &[]),
            cr("p3", &[]),
        ];
        let frame = layout(&commits);
        let o_row = frame.rows.iter().find(|r| r.sha == "o").unwrap();
        assert_eq!(o_row.parent_lanes.len(), 3);
    }

    #[test]
    fn root_commits_terminate_their_lane() {
        let commits = vec![cr("root", &[])];
        let frame = layout(&commits);
        assert_eq!(frame.rows[0].parent_lanes.len(), 0);
    }

    #[test]
    fn color_seeder_stable_within_epoch() {
        let mut s = ColorSeeder::new();
        s.allocate(0);
        let c1 = s.current(0);
        let c2 = s.current(0);
        assert_eq!(c1, c2);
    }

    #[test]
    fn passthrough_lane_drawn_when_branch_tip_waits_for_parent() {
        // c1 is on a side branch; its parent is `root` which doesn't
        // appear until two rows later. Between c1 and root, the
        // intermediate `mid` commit (on the main lane) should leave
        // c1's lane as a passthrough so the painter keeps drawing it.
        //
        //     c1
        //     | (passthrough through `mid`)
        //     mid (on lane 0)
        //     |
        //     root
        let commits = vec![
            cr("c1", &["root"]),
            cr("mid", &["root"]),
            cr("root", &[]),
        ];
        let frame = layout(&commits);
        let mid = frame.rows.iter().find(|r| r.sha == "mid").unwrap();
        // c1's lane should be visible passing through `mid`.
        assert!(
            !mid.passthrough_lanes.is_empty(),
            "expected `mid` row to have a passthrough lane for c1's branch"
        );
    }

    #[test]
    fn merged_branches_do_not_leave_dangling_lane() {
        // Reported bug: when two branches converge on the same parent
        // commit, only one lane should remain after processing — the
        // other should terminate at the merge-into-existing-lane row.
        // Previously the lane stayed alive forever and drew passthrough
        // all the way to the oldest visible commit.
        //
        //     m       (merge of c2, b1)
        //     |\
        //     c2 b1
        //     | /
        //     c1      ← both branches share this parent
        //     |
        //     root
        let commits = vec![
            cr("m",  &["c2", "b1"]),
            cr("c2", &["c1"]),
            cr("b1", &["c1"]),
            cr("c1", &["root"]),
            cr("root", &[]),
        ];
        let frame = layout(&commits);

        // After processing c1 (the convergence point), only ONE lane
        // should still be active waiting for `root`. The painter
        // reads `visible_lanes_after` to know how wide to draw.
        let c1_row = frame.rows.iter().find(|r| r.sha == "c1").unwrap();
        assert_eq!(
            c1_row.visible_lanes_after, 1,
            "after the convergence point, only one lane should remain (got {})",
            c1_row.visible_lanes_after
        );

        // At `root`, there should be NO passthrough lanes — the only
        // lane is root's own.
        let root_row = frame.rows.iter().find(|r| r.sha == "root").unwrap();
        assert!(
            root_row.passthrough_lanes.is_empty(),
            "root row should have no passthroughs; got {:?}",
            root_row.passthrough_lanes
        );
    }

    #[test]
    fn fork_commit_at_branch_origin_terminates_branch_lane() {
        // Variant: a single side branch (no merge yet) reaches its
        // origin — the commit on the side branch whose parent is on
        // the mainline. After that commit, the side lane must
        // terminate, not draw passthrough past the branch origin.
        //
        //     main2
        //     |  side  ← branch tip
        //     | /
        //     main1   ← branch origin (side's parent)
        //     |
        //     root
        let commits = vec![
            cr("main2", &["main1"]),
            cr("side",  &["main1"]),
            cr("main1", &["root"]),
            cr("root",  &[]),
        ];
        let frame = layout(&commits);

        // At main1, side's lane must have merged in — no passthrough.
        let main1_row = frame.rows.iter().find(|r| r.sha == "main1").unwrap();
        assert!(
            main1_row.passthrough_lanes.is_empty(),
            "main1 row should not have side's lane as a passthrough; got {:?}",
            main1_row.passthrough_lanes
        );
        assert_eq!(
            main1_row.visible_lanes_after, 1,
            "only mainline lane should survive past the branch origin"
        );
    }

    #[test]
    fn color_seeder_changes_on_reallocation() {
        let mut s = ColorSeeder::new();
        s.allocate(0);
        let c1 = s.current(0);
        s.allocate(0);
        let c2 = s.current(0);
        assert_ne!(c1, c2, "lane 0 should change color when re-allocated");
    }
}
