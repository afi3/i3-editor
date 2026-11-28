//! Recursive layout tree.
//!
//! The tree stores relative weights, never pixels. Rectangles are computed from
//! the current area on demand, with a hard minimum so no pane can vanish.

use std::collections::HashMap;

use ratatui::layout::Rect;

use crate::state::PaneId;

pub const MIN_PANE_ROWS: u16 = 4;
pub const MIN_PANE_COLS: u16 = 10;
pub const MAX_CHILDREN: usize = 4;
pub const MAX_TABS: usize = 8;
pub const MAX_WEIGHT: u16 = 100;
/// Row reserved for a tabbed container's tab strip.
pub const TAB_BAR_ROWS: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitMode {
    /// Children share the area along `axis`.
    Tiled,
    /// Children occupy the same area; only `active` is visible.
    Tabbed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    /// Children laid out left to right.
    LeftRight,
    /// Children laid out top to bottom.
    TopBottom,
}

impl SplitAxis {
    pub fn min_extent(self) -> u16 {
        match self {
            SplitAxis::LeftRight => MIN_PANE_COLS,
            SplitAxis::TopBottom => MIN_PANE_ROWS,
        }
    }

    pub fn extent(self, rect: Rect) -> u16 {
        match self {
            SplitAxis::LeftRight => rect.width,
            SplitAxis::TopBottom => rect.height,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayoutNode {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        mode: SplitMode,
        /// Index of the visible child when `mode` is `Tabbed`.
        active: usize,
        /// One weight per child, each in `1..=MAX_WEIGHT`.
        weights: Vec<u16>,
        children: Vec<LayoutNode>,
    },
}

/// One container tab strip, in screen coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabStrip {
    /// Path from the layout root to the tabbed split.
    pub path: Vec<usize>,
    pub rect: Rect,
    /// First leaf of every child, in tab order.
    pub leaves: Vec<PaneId>,
    /// Number of leaves inside each tab, so a tab holding a split can say so.
    pub counts: Vec<usize>,
    pub active: usize,
}

/// Boundary between two adjacent children of a split, in screen coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Separator {
    pub path: Vec<usize>,
    pub index: usize,
    pub axis: SplitAxis,
    /// Column for `LeftRight`, row for `TopBottom`.
    pub at: u16,
    pub span_start: u16,
    pub span_end: u16,
}

/// Distribute `area` along `axis` using relative weights.
pub fn split_area(area: Rect, axis: SplitAxis, weights: &[u16]) -> Vec<Rect> {
    if weights.is_empty() {
        return Vec::new();
    }
    let length = u32::from(axis.extent(area));
    let total: u32 = weights.iter().map(|weight| u32::from(*weight)).sum();
    let mut sizes = Vec::with_capacity(weights.len());
    let mut used = 0u32;
    for (index, weight) in weights.iter().enumerate() {
        let remaining = length.saturating_sub(used);
        if index + 1 == weights.len() {
            sizes.push(remaining.max(1).min(length.max(1)) as u16);
        } else {
            let share = (length * u32::from(*weight))
                .checked_div(total)
                .unwrap_or_else(|| length / weights.len() as u32);
            let size = share.max(1).min(remaining.max(1)) as u16;
            sizes.push(size);
            used += u32::from(size);
        }
    }

    let mut rects = Vec::with_capacity(sizes.len());
    let mut offset = 0u16;
    for size in sizes {
        let rect = match axis {
            SplitAxis::LeftRight => Rect {
                x: area.x.saturating_add(offset),
                y: area.y,
                width: size.min(area.width.saturating_sub(offset)),
                height: area.height,
            },
            SplitAxis::TopBottom => Rect {
                x: area.x,
                y: area.y.saturating_add(offset),
                width: area.width,
                height: size.min(area.height.saturating_sub(offset)),
            },
        };
        rects.push(rect);
        offset = offset.saturating_add(size);
    }
    rects
}

/// Bar row and content area for a tabbed container.
pub fn tabbed_regions(area: Rect) -> (Rect, Rect) {
    let bar_rows = if area.height > TAB_BAR_ROWS {
        TAB_BAR_ROWS
    } else {
        0
    };
    let bar = Rect::new(area.x, area.y, area.width, bar_rows);
    let content = Rect::new(
        area.x,
        area.y.saturating_add(bar_rows),
        area.width,
        area.height.saturating_sub(bar_rows),
    );
    (bar, content)
}

impl LayoutNode {
    pub fn leaves(&self) -> Vec<PaneId> {
        match self {
            LayoutNode::Pane(id) => vec![*id],
            LayoutNode::Split { children, .. } => {
                children.iter().flat_map(LayoutNode::leaves).collect()
            }
        }
    }

    pub fn contains(&self, target: PaneId) -> bool {
        match self {
            LayoutNode::Pane(id) => *id == target,
            LayoutNode::Split { children, .. } => children.iter().any(|c| c.contains(target)),
        }
    }

    /// Smallest extent this subtree can occupy when the parent splits along `axis`.
    pub fn min_size(&self, axis: SplitAxis) -> u16 {
        match self {
            LayoutNode::Pane(_) => axis.min_extent(),
            LayoutNode::Split {
                axis: split_axis,
                mode,
                children,
                ..
            } => match mode {
                SplitMode::Tabbed => {
                    let child_min = children
                        .iter()
                        .map(|child| child.min_size(axis))
                        .max()
                        .unwrap_or(0);
                    match axis {
                        SplitAxis::TopBottom => child_min.saturating_add(TAB_BAR_ROWS),
                        SplitAxis::LeftRight => child_min,
                    }
                }
                SplitMode::Tiled => {
                    if *split_axis == axis {
                        children.iter().map(|child| child.min_size(axis)).sum()
                    } else {
                        children
                            .iter()
                            .map(|child| child.min_size(axis))
                            .max()
                            .unwrap_or(0)
                    }
                }
            },
        }
    }

    pub fn layout(&self, area: Rect, out: &mut HashMap<PaneId, Rect>) {
        match self {
            LayoutNode::Pane(id) => {
                out.insert(*id, area);
            }
            LayoutNode::Split {
                axis,
                mode,
                active,
                weights,
                children,
            } => match mode {
                SplitMode::Tiled => {
                    let rects = split_area(area, *axis, weights);
                    for (child, rect) in children.iter().zip(rects) {
                        child.layout(rect, out);
                    }
                }
                SplitMode::Tabbed => {
                    let (_, content) = tabbed_regions(area);
                    if let Some(child) = children.get(*active) {
                        child.layout(content, out);
                    }
                }
            },
        }
    }

    /// Tab strips of every tabbed container, in screen coordinates.
    pub fn tab_strips(&self, area: Rect, path: &mut Vec<usize>, out: &mut Vec<TabStrip>) {
        let LayoutNode::Split {
            axis,
            mode,
            active,
            weights,
            children,
        } = self
        else {
            return;
        };
        if *mode == SplitMode::Tabbed {
            let (bar, _) = tabbed_regions(area);
            out.push(TabStrip {
                path: path.clone(),
                rect: bar,
                leaves: children
                    .iter()
                    .map(|child| child.leaves().first().copied().unwrap_or(PaneId(0)))
                    .collect(),
                counts: children.iter().map(|child| child.leaves().len()).collect(),
                active: *active,
            });
        }
        for (index, child) in children.iter().enumerate() {
            let child_area = match mode {
                SplitMode::Tiled => split_area(area, *axis, weights)
                    .get(index)
                    .copied()
                    .unwrap_or_default(),
                SplitMode::Tabbed => {
                    if index == *active {
                        tabbed_regions(area).1
                    } else {
                        Rect::default()
                    }
                }
            };
            if child_area.width == 0 || child_area.height == 0 {
                continue;
            }
            path.push(index);
            child.tab_strips(child_area, path, out);
            path.pop();
        }
    }

    /// Toggle the innermost split that contains `target` between tiled and tabbed.
    pub fn toggle_tabbed(&mut self, target: PaneId) -> bool {
        let LayoutNode::Split {
            mode,
            active,
            children,
            ..
        } = self
        else {
            return false;
        };
        for child in children.iter_mut() {
            if child.contains(target) && child.toggle_tabbed(target) {
                return true;
            }
        }
        let Some(index) = children.iter().position(|child| child.contains(target)) else {
            return false;
        };
        // Tiling more than `MAX_CHILDREN` panes would hand each less than the
        // documented minimum area, so a wide tabbed container stays tabbed.
        if *mode == SplitMode::Tabbed && children.len() > MAX_CHILDREN {
            return false;
        }
        *mode = match mode {
            SplitMode::Tiled => SplitMode::Tabbed,
            SplitMode::Tabbed => SplitMode::Tiled,
        };
        if *mode == SplitMode::Tabbed {
            *active = index;
        }
        true
    }

    /// Switch to the next/previous child of the innermost tabbed container.
    /// Returns the pane that receives focus.
    pub fn cycle_tab(&mut self, target: PaneId, delta: isize) -> Option<PaneId> {
        let LayoutNode::Split {
            mode,
            active,
            children,
            ..
        } = self
        else {
            return None;
        };
        for child in children.iter_mut() {
            if child.contains(target)
                && let Some(found) = child.cycle_tab(target, delta)
            {
                return Some(found);
            }
        }
        if *mode != SplitMode::Tabbed {
            return None;
        }
        let index = children.iter().position(|child| child.contains(target))?;
        let len = children.len() as isize;
        *active = ((index as isize + delta).rem_euclid(len)) as usize;
        children[*active].leaves().first().copied()
    }

    pub fn separators(&self, area: Rect, path: &mut Vec<usize>, out: &mut Vec<Separator>) {
        let LayoutNode::Split {
            axis,
            mode,
            active,
            weights,
            children,
        } = self
        else {
            return;
        };
        let rects = match mode {
            SplitMode::Tiled => split_area(area, *axis, weights),
            SplitMode::Tabbed => vec![tabbed_regions(area).1],
        };
        if *mode == SplitMode::Tiled {
            for (index, rect) in rects
                .iter()
                .take(children.len().saturating_sub(1))
                .enumerate()
            {
                let rect = *rect;
                let (at, span_start, span_end) = match axis {
                    SplitAxis::LeftRight => (
                        rect.x.saturating_add(rect.width),
                        rect.y,
                        rect.y.saturating_add(rect.height),
                    ),
                    SplitAxis::TopBottom => (
                        rect.y.saturating_add(rect.height),
                        rect.x,
                        rect.x.saturating_add(rect.width),
                    ),
                };
                out.push(Separator {
                    path: path.clone(),
                    index,
                    axis: *axis,
                    at,
                    span_start,
                    span_end,
                });
            }
        }
        for (index, child) in children.iter().enumerate() {
            if *mode == SplitMode::Tabbed && index != *active {
                continue;
            }
            let child_area = if *mode == SplitMode::Tiled {
                rects[index]
            } else {
                rects[0]
            };
            path.push(index);
            child.separators(child_area, path, out);
            path.pop();
        }
    }
}

impl LayoutNode {
    /// Insert a sibling for `target`. Returns false when the target is absent.
    /// Inside a tabbed container the new sibling becomes a tab.
    pub fn split_pane(&mut self, target: PaneId, axis: SplitAxis, new_pane: PaneId) -> bool {
        match self {
            LayoutNode::Pane(id) => {
                if *id != target {
                    return false;
                }
                let existing = LayoutNode::Pane(*id);
                *self = LayoutNode::Split {
                    axis,
                    mode: SplitMode::Tiled,
                    active: 0,
                    weights: vec![1, 1],
                    children: vec![existing, LayoutNode::Pane(new_pane)],
                };
                true
            }
            LayoutNode::Split {
                axis: current,
                mode,
                active,
                weights,
                children,
            } => {
                let limit = if *mode == SplitMode::Tabbed {
                    MAX_TABS
                } else {
                    MAX_CHILDREN
                };
                let axis_matches = *mode == SplitMode::Tabbed || *current == axis;
                if axis_matches
                    && children.len() < limit
                    && let Some(index) = children
                        .iter()
                        .position(|child| matches!(child, LayoutNode::Pane(id) if *id == target))
                {
                    let weight = weights[index].clamp(1, MAX_WEIGHT);
                    weights.insert(index + 1, weight);
                    children.insert(index + 1, LayoutNode::Pane(new_pane));
                    if *mode == SplitMode::Tabbed {
                        *active = index + 1;
                    }
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.split_pane(target, axis, new_pane))
            }
        }
    }

    /// Remove `target` and collapse redundant splits. Refuses the last pane.
    pub fn remove_pane(&mut self, target: PaneId) -> bool {
        if matches!(self, LayoutNode::Pane(id) if *id == target) {
            return false;
        }
        let removed = Self::remove_from_children(self, target);
        if removed {
            self.collapse();
        }
        removed
    }

    fn remove_from_children(node: &mut LayoutNode, target: PaneId) -> bool {
        let LayoutNode::Split {
            active,
            weights,
            children,
            ..
        } = node
        else {
            return false;
        };
        if let Some(index) = children
            .iter()
            .position(|child| matches!(child, LayoutNode::Pane(id) if *id == target))
        {
            let removed_before_active = index < *active;
            children.remove(index);
            weights.remove(index);
            if children.is_empty() {
                *active = 0;
            } else {
                if removed_before_active {
                    *active -= 1;
                }
                *active = (*active).min(children.len() - 1);
            }
            return true;
        }
        for child in children.iter_mut() {
            if Self::remove_from_children(child, target) {
                return true;
            }
        }
        false
    }

    /// Exchange two leaves without touching the tree shape, which is exactly what
    /// "move this pane there" means for a tiled layout.
    pub fn swap_panes(&mut self, first: PaneId, second: PaneId) -> bool {
        if first == second || !self.contains(first) || !self.contains(second) {
            return false;
        }
        Self::swap_walk(self, first, second, &mut false, &mut false)
    }

    fn swap_walk(
        node: &mut LayoutNode,
        first: PaneId,
        second: PaneId,
        saw_first: &mut bool,
        saw_second: &mut bool,
    ) -> bool {
        match node {
            LayoutNode::Pane(id) => {
                if *id == first {
                    *id = second;
                    *saw_first = true;
                } else if *id == second {
                    *id = first;
                    *saw_second = true;
                }
            }
            LayoutNode::Split { children, .. } => {
                for child in children.iter_mut() {
                    Self::swap_walk(child, first, second, saw_first, saw_second);
                }
            }
        }
        *saw_first && *saw_second
    }

    pub fn collapse(&mut self) {
        if let LayoutNode::Split {
            active, children, ..
        } = self
        {
            for child in children.iter_mut() {
                child.collapse();
            }
            if children.len() == 1 {
                let only = children.pop().expect("one child");
                *self = only;
            } else if !children.is_empty() {
                *active = (*active).min(children.len() - 1);
            }
        }
    }

    pub fn node_at(&mut self, path: &[usize]) -> Option<&mut LayoutNode> {
        let mut node = self;
        for index in path {
            match node {
                LayoutNode::Split { children, .. } => node = children.get_mut(*index)?,
                LayoutNode::Pane(_) => return None,
            }
        }
        Some(node)
    }

    pub fn weights_at(&mut self, path: &[usize]) -> Option<&mut Vec<u16>> {
        match self.node_at(path)? {
            LayoutNode::Split { weights, .. } => Some(weights),
            LayoutNode::Pane(_) => None,
        }
    }

    /// Apply a separator drag. `cells` are the current sizes of every child.
    pub fn resize(&mut self, path: &[usize], index: usize, cells: &[u16]) -> bool {
        let Some(weights) = self.weights_at(path) else {
            return false;
        };
        if index + 1 >= weights.len() || cells.len() != weights.len() {
            return false;
        }
        let max_cell = cells.iter().copied().max().unwrap_or(1).max(1);
        for (weight, cell) in weights.iter_mut().zip(cells) {
            let scaled = u32::from(*cell) * u32::from(MAX_WEIGHT) / u32::from(max_cell);
            *weight = (scaled as u16).clamp(1, MAX_WEIGHT);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(axis: SplitAxis, weights: &[u16], children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode::Split {
            axis,
            mode: SplitMode::Tiled,
            active: 0,
            weights: weights.to_vec(),
            children,
        }
    }

    fn tabbed(children: Vec<LayoutNode>, active: usize) -> LayoutNode {
        LayoutNode::Split {
            axis: SplitAxis::LeftRight,
            mode: SplitMode::Tabbed,
            active,
            weights: vec![1; children.len()],
            children,
        }
    }

    #[test]
    fn split_area_fills_the_parent_exactly() {
        let area = Rect::new(0, 0, 80, 24);
        let rects = split_area(area, SplitAxis::LeftRight, &[1, 1]);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width + rects[1].width, 80);
        assert_eq!(rects[1].x, 40);

        let rows = split_area(area, SplitAxis::TopBottom, &[1, 2, 1]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().map(|r| r.height).sum::<u16>(), 24);
    }

    #[test]
    fn min_size_sums_along_the_split_axis_and_maxes_across_it() {
        let tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![
                LayoutNode::Pane(PaneId(1)),
                split(
                    SplitAxis::TopBottom,
                    &[1, 1],
                    vec![LayoutNode::Pane(PaneId(2)), LayoutNode::Pane(PaneId(3))],
                ),
            ],
        );
        assert_eq!(tree.min_size(SplitAxis::LeftRight), MIN_PANE_COLS * 2);
        assert_eq!(tree.min_size(SplitAxis::TopBottom), MIN_PANE_ROWS * 2);
    }

    #[test]
    fn splitting_adds_a_sibling_on_the_same_axis() {
        let mut tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
        );
        assert!(tree.split_pane(PaneId(2), SplitAxis::LeftRight, PaneId(3)));
        assert_eq!(tree.leaves(), vec![PaneId(1), PaneId(2), PaneId(3)]);
        let LayoutNode::Split { weights, .. } = &tree else {
            panic!("expected split");
        };
        assert_eq!(weights.len(), 3);
    }

    #[test]
    fn splitting_the_opposite_axis_nests() {
        let mut tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
        );
        assert!(tree.split_pane(PaneId(1), SplitAxis::TopBottom, PaneId(3)));
        let LayoutNode::Split { children, .. } = &tree else {
            panic!("expected root split");
        };
        assert!(matches!(children[0], LayoutNode::Split { .. }));
    }

    #[test]
    fn removing_a_sibling_collapses_the_split() {
        let mut tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
        );
        assert!(tree.remove_pane(PaneId(2)));
        assert_eq!(tree, LayoutNode::Pane(PaneId(1)));
    }

    #[test]
    fn the_last_pane_cannot_be_removed() {
        let mut tree = LayoutNode::Pane(PaneId(1));
        assert!(!tree.remove_pane(PaneId(1)));
        assert_eq!(tree, LayoutNode::Pane(PaneId(1)));
    }

    #[test]
    fn layout_maps_every_leaf_into_the_area() {
        let tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![
                LayoutNode::Pane(PaneId(1)),
                split(
                    SplitAxis::TopBottom,
                    &[1, 1],
                    vec![LayoutNode::Pane(PaneId(2)), LayoutNode::Pane(PaneId(3))],
                ),
            ],
        );
        let area = Rect::new(0, 0, 80, 24);
        let mut rects = HashMap::new();
        tree.layout(area, &mut rects);
        assert_eq!(rects.len(), 3);
        for rect in rects.values() {
            assert!(rect.width > 0 && rect.height > 0);
            assert!(rect.x + rect.width <= area.x + area.width);
            assert!(rect.y + rect.height <= area.y + area.height);
        }
        assert_ne!(rects[&PaneId(2)], rects[&PaneId(3)]);
    }

    #[test]
    fn swapping_leaves_preserves_the_tree_shape() {
        let mut tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
        );
        assert!(tree.swap_panes(PaneId(1), PaneId(2)));
        assert_eq!(tree.leaves(), vec![PaneId(2), PaneId(1)]);
        let LayoutNode::Split { weights, .. } = &tree else {
            panic!("expected split");
        };
        assert_eq!(weights, &[1, 1]);
        assert!(!tree.swap_panes(PaneId(1), PaneId(9)));
        assert!(!tree.swap_panes(PaneId(1), PaneId(1)));
    }

    #[test]
    fn separators_are_reported_between_children() {
        let tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
        );
        let mut out = Vec::new();
        tree.separators(Rect::new(0, 0, 80, 24), &mut Vec::new(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].at, 40);
        assert_eq!(out[0].axis, SplitAxis::LeftRight);
    }

    #[test]
    fn tabbed_container_lays_out_only_the_active_tab() {
        let tree = tabbed(
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
            1,
        );
        let area = Rect::new(2, 3, 60, 20);
        let mut rects = HashMap::new();
        tree.layout(area, &mut rects);
        assert_eq!(rects.len(), 1);
        let rect = rects[&PaneId(2)];
        assert_eq!(rect.y, area.y + TAB_BAR_ROWS);
        assert_eq!(rect.height, area.height - TAB_BAR_ROWS);
        assert_eq!(rect.width, area.width);
    }

    #[test]
    fn tab_strips_report_the_bar_and_its_tabs() {
        let tree = tabbed(
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
            0,
        );
        let area = Rect::new(0, 0, 80, 24);
        let mut out = Vec::new();
        tree.tab_strips(area, &mut Vec::new(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].rect.height, TAB_BAR_ROWS);
        assert_eq!(out[0].leaves, vec![PaneId(1), PaneId(2)]);
        assert_eq!(out[0].active, 0);
        assert_eq!(out[0].path, Vec::<usize>::new());
    }

    #[test]
    fn tabbed_minimum_uses_the_largest_child_plus_the_bar() {
        let tree = tabbed(
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
            0,
        );
        assert_eq!(tree.min_size(SplitAxis::LeftRight), MIN_PANE_COLS);
        assert_eq!(
            tree.min_size(SplitAxis::TopBottom),
            MIN_PANE_ROWS + TAB_BAR_ROWS
        );
    }

    #[test]
    fn toggling_tabbed_uses_the_innermost_split() {
        let mut tree = split(
            SplitAxis::LeftRight,
            &[1, 1],
            vec![
                LayoutNode::Pane(PaneId(1)),
                split(
                    SplitAxis::TopBottom,
                    &[1, 1],
                    vec![LayoutNode::Pane(PaneId(2)), LayoutNode::Pane(PaneId(3))],
                ),
            ],
        );
        assert!(tree.toggle_tabbed(PaneId(3)));
        let LayoutNode::Split { children, .. } = &tree else {
            panic!("root split");
        };
        let LayoutNode::Split { mode, active, .. } = &children[1] else {
            panic!("nested split");
        };
        assert_eq!(*mode, SplitMode::Tabbed);
        assert_eq!(*active, 1);
        assert!(tree.toggle_tabbed(PaneId(3)));
        let LayoutNode::Split { children, .. } = &tree else {
            panic!("root split");
        };
        let LayoutNode::Split { mode, .. } = &children[1] else {
            panic!("nested split");
        };
        assert_eq!(*mode, SplitMode::Tiled);
    }

    #[test]
    fn a_tabbed_split_with_too_many_children_stays_tabbed() {
        let children: Vec<LayoutNode> = (0..5)
            .map(|index| LayoutNode::Pane(PaneId(index)))
            .collect();
        let mut tree = split(SplitAxis::LeftRight, &[1, 1, 1, 1, 1], children);
        assert!(tree.toggle_tabbed(PaneId(0)), "tiling is allowed");
        assert!(
            !tree.toggle_tabbed(PaneId(0)),
            "five children cannot be tiled"
        );
    }

    #[test]
    fn cycling_tabs_moves_the_active_index() {
        let mut tree = tabbed(
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
            0,
        );
        assert_eq!(tree.cycle_tab(PaneId(1), 1), Some(PaneId(2)));
        let LayoutNode::Split { active, .. } = &tree else {
            panic!("tabbed");
        };
        assert_eq!(*active, 1);
        assert_eq!(tree.cycle_tab(PaneId(2), -1), Some(PaneId(1)));
    }

    #[test]
    fn splitting_inside_a_tabbed_container_adds_a_tab() {
        let mut tree = tabbed(
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
            0,
        );
        assert!(tree.split_pane(PaneId(1), SplitAxis::TopBottom, PaneId(3)));
        let LayoutNode::Split {
            mode,
            active,
            children,
            ..
        } = &tree
        else {
            panic!("tabbed");
        };
        assert_eq!(*mode, SplitMode::Tabbed);
        assert_eq!(*active, 1);
        assert_eq!(children.len(), 3);
        assert_eq!(tree.leaves(), vec![PaneId(1), PaneId(3), PaneId(2)]);
    }

    #[test]
    fn closing_a_tab_collapses_a_one_tab_container() {
        let mut tree = tabbed(
            vec![LayoutNode::Pane(PaneId(1)), LayoutNode::Pane(PaneId(2))],
            1,
        );
        assert!(tree.remove_pane(PaneId(1)));
        assert_eq!(tree, LayoutNode::Pane(PaneId(2)));
    }
}
