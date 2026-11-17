//! Mouse: clicks place the cursor, the wheel scrolls panes and the explorer,
//! dragging a split border resizes it.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::editor::text as textutil;
use crate::state::{Focus, PaneId};
use crate::workspace::SplitAxis;
use crate::workspace::layout::{LayoutNode, Separator, SplitMode, split_area, tabbed_regions};

use super::App;

#[derive(Debug)]
pub struct Drag {
    pub path: Vec<usize>,
    pub index: usize,
    pub axis: SplitAxis,
    pub cells: Vec<u16>,
    pub mins: Vec<u16>,
    pub start: u16,
}

impl App {
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.on_left_click(mouse),
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(drag) = self.drag.take() {
                    self.resize_to(drag, mouse.column, mouse.row);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            MouseEventKind::ScrollUp => self.on_wheel(mouse.column, mouse.row, -1),
            MouseEventKind::ScrollDown => self.on_wheel(mouse.column, mouse.row, 1),
            MouseEventKind::ScrollLeft => self.on_horizontal_wheel(mouse.column, mouse.row, -1),
            MouseEventKind::ScrollRight => self.on_horizontal_wheel(mouse.column, mouse.row, 1),
            _ => {}
        }
    }

    fn on_left_click(&mut self, mouse: MouseEvent) {
        let extend = mouse.modifiers.contains(KeyModifiers::SHIFT);
        if let Some((path, index)) = self.tab_at(mouse.column, mouse.row) {
            self.activate_tab(&path, index);
            return;
        }
        if let Some(drag) = self.drag_at(mouse.column, mouse.row) {
            self.drag = Some(drag);
            return;
        }
        if self.state.shell.is_some() && contains(self.state.ui.shell, mouse.column, mouse.row) {
            self.state.focus = Focus::Shell;
            return;
        }
        if self.state.explorer.visible && contains(self.state.ui.explorer, mouse.column, mouse.row)
        {
            self.click_explorer(mouse.column, mouse.row);
            return;
        }
        if let Some(pane_id) = self.pane_at(mouse.column, mouse.row) {
            self.state.focus = Focus::Pane(pane_id);
            self.state.workspaces.active_mut().focus = pane_id;
            self.place_cursor(pane_id, mouse.column, mouse.row, extend);
        }
    }

    fn click_explorer(&mut self, column: u16, row: u16) {
        self.state.focus = Focus::Explorer;
        let Some(geometry) = crate::ui::overlay::explorer_geometry(&self.state) else {
            return;
        };
        if !contains(geometry.area, column, row) {
            return;
        }
        let index = geometry.scroll + usize::from(row.saturating_sub(geometry.area.y));
        if index >= self.state.explorer.rows.len() {
            return;
        }
        self.state.explorer.selected = index;
        let now = Instant::now();
        let double = matches!(
            self.last_explorer_click,
            Some((last, at)) if last == index && now.duration_since(at) < Duration::from_millis(400)
        );
        self.last_explorer_click = if double { None } else { Some((index, now)) };
        if double {
            let result = self.explorer_open();
            self.finish(result);
        }
    }

    fn on_wheel(&mut self, column: u16, row: u16, steps: isize) {
        if self.state.shell.is_some() && contains(self.state.ui.shell, column, row) {
            self.scroll_shell(-steps * 3);
            return;
        }
        if self.state.explorer.visible && contains(self.state.ui.explorer, column, row) {
            self.state.focus = Focus::Explorer;
            self.state.explorer.move_selection(steps * 3);
            return;
        }
        if let Some(pane_id) = self.pane_at(column, row) {
            self.scroll_pane(pane_id, steps * 3, 0);
        }
    }

    fn on_horizontal_wheel(&mut self, column: u16, row: u16, steps: isize) {
        if self.state.explorer.visible && contains(self.state.ui.explorer, column, row) {
            return;
        }
        if let Some(pane_id) = self.pane_at(column, row) {
            self.scroll_pane(pane_id, 0, steps * 4);
        }
    }

    fn pane_at(&self, column: u16, row: u16) -> Option<PaneId> {
        self.state
            .ui
            .pane_rects
            .iter()
            .find(|(_, rect)| contains(**rect, column, row))
            .map(|(id, _)| *id)
    }

    /// Move the cursor of `pane_id` to the clicked cell.
    fn place_cursor(&mut self, pane_id: PaneId, column: u16, row: u16, extend: bool) {
        let placement = {
            let Some(pane) = self.state.panes.get(&pane_id) else {
                return;
            };
            let Some(rect) = self.state.ui.pane_rects.get(&pane_id).copied() else {
                return;
            };
            let Some(document_id) = pane.active else {
                return;
            };
            let Some(document) = self.state.documents.get(&document_id) else {
                return;
            };
            let Some(geometry) = crate::ui::pane::text_geometry(&self.state, pane, document, rect)
            else {
                return;
            };
            if !contains(geometry.area, column, row) {
                return;
            }
            let line_offset = usize::from(row.saturating_sub(geometry.area.y));
            let line = (pane.scroll_top + line_offset).min(document.line_count().saturating_sub(1));
            let text = document.line_text(line);
            let tab_width = self.state.config.editor.tab_size.max(1);
            let cell = column
                .saturating_sub(geometry.area.x.saturating_add(geometry.gutter))
                .saturating_add(pane.scroll_left);
            let offset = textutil::char_at_cell(&text, cell, tab_width);
            (document.char_of_line(line) + offset, pane.cursor)
        };
        let (cursor, previous) = placement;
        let Some(pane) = self.state.panes.get_mut(&pane_id) else {
            return;
        };
        if extend && pane.anchor.is_none() {
            pane.anchor = Some(previous);
        } else if !extend {
            pane.anchor = None;
        }
        pane.cursor = cursor;
        pane.target_col = None;
    }

    /// Scroll a pane without moving its cursor.
    fn scroll_pane(&mut self, pane_id: PaneId, vertical: isize, horizontal: isize) {
        let limits = {
            let Some(pane) = self.state.panes.get(&pane_id) else {
                return;
            };
            let Some(rect) = self.state.ui.pane_rects.get(&pane_id).copied() else {
                return;
            };
            let Some(document_id) = pane.active else {
                return;
            };
            let Some(document) = self.state.documents.get(&document_id) else {
                return;
            };
            let Some(geometry) = crate::ui::pane::text_geometry(&self.state, pane, document, rect)
            else {
                return;
            };
            let rows = usize::from(geometry.area.height).max(1);
            let max_top = document.line_count().saturating_sub(rows);
            let tab_width = self.state.config.editor.tab_size.max(1);
            let last = (pane.scroll_top + rows).min(document.line_count());
            let max_width = (pane.scroll_top..last)
                .map(|line| textutil::cells(&document.line_text(line), tab_width, 0))
                .max()
                .unwrap_or(0);
            (max_top, max_width, pane.scroll_top, pane.scroll_left)
        };
        let (max_top, max_width, top, left) = limits;
        let top = (top as isize + vertical).clamp(0, max_top as isize) as usize;
        let left = (i64::from(left) + horizontal as i64).clamp(0, i64::from(max_width)) as u16;
        let Some(pane) = self.state.panes.get_mut(&pane_id) else {
            return;
        };
        pane.scroll_top = top;
        pane.scroll_left = left;
        self.state.focus = Focus::Pane(pane_id);
        self.state.workspaces.active_mut().focus = pane_id;
    }

    fn tab_at(&self, column: u16, row: u16) -> Option<(Vec<usize>, usize)> {
        for bar in &self.state.ui.tab_bars {
            let rows = bar.rect.y..bar.rect.y.saturating_add(bar.rect.height);
            if !rows.contains(&row) {
                continue;
            }
            for (index, tab) in bar.tabs.iter().enumerate() {
                if column >= tab.start && column < tab.end {
                    return Some((bar.path.clone(), index));
                }
            }
        }
        None
    }

    fn drag_at(&self, column: u16, row: u16) -> Option<Drag> {
        let area = self.state.ui.panes;
        if !contains(area, column, row) {
            return None;
        }
        let layout = &self.state.workspaces.active().layout;
        let mut separators = Vec::new();
        layout.separators(area, &mut Vec::new(), &mut separators);
        let separator = separators
            .into_iter()
            .find(|separator| hit(separator, column, row))?;
        let (axis, rects, mins) = split_geometry(layout, area, &separator.path)?;
        let cells = rects
            .iter()
            .map(|rect| axis.extent(*rect))
            .collect::<Vec<_>>();
        let start = match axis {
            SplitAxis::LeftRight => column,
            SplitAxis::TopBottom => row,
        };
        Some(Drag {
            path: separator.path,
            index: separator.index,
            axis,
            cells,
            mins,
            start,
        })
    }

    fn resize_to(&mut self, mut drag: Drag, column: u16, row: u16) {
        let position = match drag.axis {
            SplitAxis::LeftRight => column,
            SplitAxis::TopBottom => row,
        };
        let index = drag.index;
        if index + 1 >= drag.cells.len() || index + 1 >= drag.mins.len() {
            return;
        }
        let delta = i64::from(position) - i64::from(drag.start);
        let total = i64::from(drag.cells[index]) + i64::from(drag.cells[index + 1]);
        let min_left = i64::from(drag.mins[index]);
        let min_right = i64::from(drag.mins[index + 1]);
        let upper = (total - min_right).max(min_left);
        let left = (i64::from(drag.cells[index]) + delta).clamp(min_left, upper);
        drag.cells[index] = left as u16;
        drag.cells[index + 1] = (total - left) as u16;

        let path = drag.path.clone();
        let cells = drag.cells.clone();
        self.state
            .workspaces
            .active_mut()
            .layout
            .resize(&path, index, &cells);
        self.drag = Some(drag);
    }
}

fn split_geometry(
    layout: &LayoutNode,
    area: Rect,
    path: &[usize],
) -> Option<(SplitAxis, Vec<Rect>, Vec<u16>)> {
    let mut node = layout;
    let mut rect = area;
    for index in path {
        let LayoutNode::Split {
            axis,
            mode,
            active,
            weights,
            children,
        } = node
        else {
            return None;
        };
        rect = match mode {
            SplitMode::Tiled => *split_area(rect, *axis, weights).get(*index)?,
            SplitMode::Tabbed => {
                if index != active {
                    return None;
                }
                tabbed_regions(rect).1
            }
        };
        node = children.get(*index)?;
    }
    if let LayoutNode::Split {
        axis,
        mode: SplitMode::Tiled,
        weights,
        children,
        ..
    } = node
    {
        let rects = split_area(rect, *axis, weights);
        let mins = children.iter().map(|child| child.min_size(*axis)).collect();
        return Some((*axis, rects, mins));
    }
    None
}

fn contains(rect: Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn hit(separator: &Separator, column: u16, row: u16) -> bool {
    let perpendicular = match separator.axis {
        SplitAxis::LeftRight => row,
        SplitAxis::TopBottom => column,
    };
    let along = match separator.axis {
        SplitAxis::LeftRight => column,
        SplitAxis::TopBottom => row,
    };
    perpendicular >= separator.span_start
        && perpendicular < separator.span_end
        && along.abs_diff(separator.at) <= 1
}
