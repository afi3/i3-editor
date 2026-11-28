//! Mouse: clicks place the cursor, the wheel scrolls panes and the explorer,
//! dragging a split border resizes it.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders};

use crate::editor::text as textutil;
use crate::state::{AppState, DocumentId, Focus, InputMode, PaneId};
use crate::ui::pane::{
    DocumentTab, ScrollbarGeometry, document_tabs, scrollbar_geometry, tab_strip_geometry,
};
use crate::workspace::SplitAxis;
use crate::workspace::layout::{LayoutNode, Separator, SplitMode, split_area, tabbed_regions};

use super::App;

#[derive(Debug)]
pub struct ScrollbarDrag {
    pub pane: PaneId,
    /// Rows from the top of the thumb where the drag started.
    pub grab: u16,
}

#[derive(Debug)]
pub struct Drag {
    pub path: Vec<usize>,
    pub index: usize,
    pub axis: SplitAxis,
    pub cells: Vec<u16>,
    pub mins: Vec<u16>,
    pub start: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DocumentTabHit {
    Activate(DocumentId),
    Close(DocumentId),
    /// Scroll the strip one tab left (`-1`) or right (`1`).
    Scroll(isize),
}

impl App {
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        // Read-only overlays are modal for the keyboard; they own the wheel and
        // swallow clicks so the buffer underneath cannot be edited through them.
        if self.handle_prompt_mouse(mouse)
            || self.handle_overlay_mouse(mouse)
            || (self.drag.is_none() && self.forward_shell_mouse(mouse))
        {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.on_left_click(mouse),
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.scrollbar_drag.is_some() {
                    self.drag_scrollbar(mouse.row);
                } else if let Some(drag) = self.drag.take() {
                    self.resize_to(drag, mouse.column, mouse.row);
                } else if let Some((pane_id, line, column)) = self.block_anchor {
                    self.extend_block(pane_id, line, column, mouse.column, mouse.row);
                } else if let Some(pane_id) = self.selecting {
                    self.place_cursor(pane_id, mouse.column, mouse.row, true);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                self.scrollbar_drag = None;
                self.selecting = None;
                self.block_anchor = None;
            }
            MouseEventKind::ScrollUp => self.on_wheel(mouse.column, mouse.row, -1),
            MouseEventKind::ScrollDown => self.on_wheel(mouse.column, mouse.row, 1),
            MouseEventKind::ScrollLeft => self.on_horizontal_wheel(mouse.column, mouse.row, -1),
            MouseEventKind::ScrollRight => self.on_horizontal_wheel(mouse.column, mouse.row, 1),
            _ => {}
        }
    }

    /// Send a mouse event to the shell program when it asked for mouse
    /// reporting. The panel body is the area inside its border; coordinates
    /// become zero-based before encoding.
    fn forward_shell_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(inner) = shell_inner(self.state.ui.shell) else {
            return false;
        };
        if !contains(inner, mouse.column, mouse.row) {
            return false;
        }
        let Some(shell) = self.state.terminals.active() else {
            return false;
        };
        if !shell.reports_mouse() {
            return false;
        }
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.state.focus = Focus::Shell;
        }
        let column = mouse.column.saturating_sub(inner.x);
        let row = mouse.row.saturating_sub(inner.y);
        self.state
            .terminals
            .active_mut()
            .is_some_and(|shell| shell.send_mouse(mouse, column, row))
    }

    fn on_left_click(&mut self, mouse: MouseEvent) {
        let extend = mouse.modifiers.contains(KeyModifiers::SHIFT);
        self.selecting = None;
        if self.state.ai.visible && contains(self.state.ui.ai, mouse.column, mouse.row) {
            self.state.ai.focused = true;
            return;
        }
        self.state.ai.focused = false;
        if contains(self.state.ui.workspace_bar, mouse.column, mouse.row) {
            if let Some(index) = self
                .state
                .ui
                .workspace_hits
                .iter()
                .find(|hit| mouse.column >= hit.start && mouse.column < hit.end)
                .map(|hit| hit.index)
            {
                let result = self.switch_workspace(index);
                self.finish(result);
            }
            return;
        }
        if self.click_document_tab(mouse.column, mouse.row) {
            return;
        }
        if let Some((path, index)) = self.tab_at(mouse.column, mouse.row) {
            self.activate_tab(&path, index);
            return;
        }
        if self.click_scrollbar(mouse.column, mouse.row) {
            return;
        }
        if let Some(drag) = self.drag_at(mouse.column, mouse.row) {
            self.drag = Some(drag);
            return;
        }
        if !self.state.terminals.is_empty()
            && contains(self.state.ui.shell, mouse.column, mouse.row)
        {
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
            if mouse.modifiers.contains(KeyModifiers::ALT) {
                if let Some((line, column, _)) =
                    self.cursor_location(pane_id, mouse.column, mouse.row)
                {
                    self.start_block(pane_id, line, column);
                    self.selecting = Some(pane_id);
                }
            } else if self.place_cursor(pane_id, mouse.column, mouse.row, extend) {
                self.selecting = Some(pane_id);
            }
        }
    }

    /// Read-only overlays own the wheel and swallow clicks. The draw order is
    /// diff, popups, help, MCP, rules, trace, hunks, prompt, so the last open
    /// overlay wins.
    fn handle_overlay_mouse(&mut self, mouse: MouseEvent) -> bool {
        let steps = match mouse.kind {
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::ScrollDown => 3,
            _ => 0,
        };
        if self.state.hunks.is_some() {
            if steps != 0 {
                self.scroll_hunks(steps);
            }
            return true;
        }
        self.scroll_panel_overlays(steps)
    }

    fn scroll_panel_overlays(&mut self, steps: isize) -> bool {
        let mut open = false;
        if let Some(view) = self.state.mcp_view.as_mut() {
            open = true;
            if steps != 0 {
                super::help::scroll_view(view, steps);
            }
        }
        if let Some(view) = self.state.rules_view.as_mut() {
            open = true;
            if steps != 0 {
                super::help::scroll_view(view, steps);
            }
        }
        if let Some(view) = self.state.trace_view.as_mut() {
            open = true;
            if steps != 0 {
                super::help::scroll_view(view, steps);
            }
        }
        if let Some(view) = self.state.help.as_mut() {
            open = true;
            if steps != 0 {
                super::help::scroll_view(view, steps);
            }
        }
        if let Some(view) = self.state.git_diff.as_mut() {
            open = true;
            if steps != 0 {
                super::help::scroll_view(view, steps);
            }
        }
        open
    }

    /// Prompt overlay owns input; only the wheel may scroll its list.
    fn handle_prompt_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !self.state.mode.is_prompt() {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_prompt(-3),
            MouseEventKind::ScrollDown => self.scroll_prompt(3),
            _ => {}
        }
        true
    }

    fn scroll_prompt(&mut self, steps: isize) {
        let InputMode::Prompt(prompt) = &self.state.mode else {
            return;
        };
        if prompt.kind == crate::input::PromptKind::Find {
            self.move_search_selection(steps);
            return;
        }
        if let InputMode::Prompt(prompt) = &mut self.state.mode {
            prompt.move_selection(steps);
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
        if !self.state.terminals.is_empty() && contains(self.state.ui.shell, column, row) {
            self.scroll_shell(-steps * 3);
            return;
        }
        if self.state.explorer.visible && contains(self.state.ui.explorer, column, row) {
            self.state.focus = Focus::Explorer;
            self.state.explorer.move_selection(steps * 3);
            return;
        }
        if let Some((pane_id, _)) = self.document_tab_hit(column, row) {
            self.scroll_document_tabs(pane_id, steps.signum());
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
        if let Some((pane_id, _)) = self.document_tab_hit(column, row) {
            self.scroll_document_tabs(pane_id, steps.signum());
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

    /// Line, char column within the line, and absolute char position under the
    /// mouse, honoring folds.
    fn cursor_location(
        &self,
        pane_id: PaneId,
        column: u16,
        row: u16,
    ) -> Option<(usize, usize, usize)> {
        let pane = self.state.panes.get(&pane_id)?;
        let rect = self.state.ui.pane_rects.get(&pane_id).copied()?;
        let document = self.state.documents.get(&pane.active?)?;
        let geometry = crate::ui::pane::text_geometry(&self.state, pane, document, rect)?;
        if !contains(geometry.area, column, row) {
            return None;
        }
        let line_offset = usize::from(row.saturating_sub(geometry.area.y));
        let tab_width = document.indent.tab_width.max(1);
        if self.state.config.editor.word_wrap {
            let rows = textutil::visual_rows(
                document,
                pane.scroll_top,
                geometry.text_width,
                tab_width,
                line_offset + 1,
            );
            let row = rows.get(line_offset)?;
            let full = document.line_text(row.line);
            let segment: String = full
                .chars()
                .skip(row.start)
                .take(row.end - row.start)
                .collect();
            let cell = column.saturating_sub(geometry.area.x.saturating_add(geometry.gutter));
            let offset = row.start + textutil::char_at_cell(&segment, cell, tab_width);
            return Some((row.line, offset, document.char_of_line(row.line) + offset));
        }
        let line = (pane.scroll_top..document.line_count())
            .filter(|line| !document.is_hidden(*line))
            .nth(line_offset)
            .unwrap_or_else(|| document.line_count().saturating_sub(1));
        let text = document.line_text(line);
        let cell = column
            .saturating_sub(geometry.area.x.saturating_add(geometry.gutter))
            .saturating_add(pane.scroll_left);
        let offset = textutil::char_at_cell(&text, cell, tab_width);
        Some((line, offset, document.char_of_line(line) + offset))
    }

    /// Move the cursor of `pane_id` to the clicked cell.
    fn place_cursor(&mut self, pane_id: PaneId, column: u16, row: u16, extend: bool) -> bool {
        let Some((_, _, cursor)) = self.cursor_location(pane_id, column, row) else {
            return false;
        };
        let Some(pane) = self.state.panes.get_mut(&pane_id) else {
            return false;
        };
        let previous = pane.cursor;
        pane.extra.clear();
        if extend && pane.anchor.is_none() {
            pane.anchor = Some(previous);
        } else if !extend {
            pane.anchor = None;
        }
        pane.cursor = cursor;
        pane.target_col = None;
        true
    }

    fn start_block(&mut self, pane_id: PaneId, line: usize, column: usize) {
        self.block_anchor = Some((pane_id, line, column));
        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let Some(pane) = panes.get_mut(&pane_id) else {
            return;
        };
        let Some(id) = pane.active else {
            return;
        };
        let Some(document) = documents.get(&id) else {
            return;
        };
        pane.column_selection(document, line, column, line, column);
        self.state.focus = Focus::Pane(pane_id);
    }

    fn extend_block(
        &mut self,
        pane_id: PaneId,
        anchor_line: usize,
        anchor_column: usize,
        column: u16,
        row: u16,
    ) {
        let Some((line, offset, _)) = self.cursor_location(pane_id, column, row) else {
            return;
        };
        let AppState {
            documents, panes, ..
        } = &mut self.state;
        let Some(pane) = panes.get_mut(&pane_id) else {
            return;
        };
        let Some(id) = pane.active else {
            return;
        };
        let Some(document) = documents.get(&id) else {
            return;
        };
        pane.column_selection(document, anchor_line, anchor_column, line, offset);
    }

    fn click_document_tab(&mut self, column: u16, row: u16) -> bool {
        let Some((pane_id, hit)) = self.document_tab_hit(column, row) else {
            return false;
        };
        self.state.focus = Focus::Pane(pane_id);
        self.state.workspaces.active_mut().focus = pane_id;
        match hit {
            DocumentTabHit::Activate(id) => {
                let result = self.activate_document(id);
                self.finish(result);
            }
            DocumentTabHit::Close(id) => {
                let result = self.close_document_id(id);
                self.finish(result);
            }
            DocumentTabHit::Scroll(direction) => {
                self.scroll_document_tabs(pane_id, direction);
            }
        }
        true
    }

    fn click_scrollbar(&mut self, column: u16, row: u16) -> bool {
        let Some((pane_id, scrollbar)) = self.scrollbar_at(column, row) else {
            return false;
        };
        self.begin_scrollbar_drag(pane_id, &scrollbar, row);
        true
    }

    fn scrollbar_at(&self, column: u16, row: u16) -> Option<(PaneId, ScrollbarGeometry)> {
        for (pane_id, pane) in &self.state.panes {
            let Some(rect) = self.state.ui.pane_rects.get(pane_id) else {
                continue;
            };
            let Some(document) = pane.active.and_then(|id| self.state.documents.get(&id)) else {
                continue;
            };
            let Some(scrollbar) = scrollbar_geometry(&self.state, pane, document, *rect) else {
                continue;
            };
            if contains(scrollbar.track, column, row) {
                return Some((*pane_id, scrollbar));
            }
        }
        None
    }

    fn begin_scrollbar_drag(&mut self, pane_id: PaneId, scrollbar: &ScrollbarGeometry, row: u16) {
        let thumb_top = scrollbar.track.y.saturating_add(scrollbar.thumb_start);
        let grab = if row >= thumb_top && row < thumb_top.saturating_add(scrollbar.thumb_len) {
            row.saturating_sub(thumb_top)
        } else {
            scrollbar.thumb_len / 2
        };
        self.scrollbar_drag = Some(ScrollbarDrag {
            pane: pane_id,
            grab,
        });
        self.state.focus = Focus::Pane(pane_id);
        self.state.workspaces.active_mut().focus = pane_id;
        self.set_scrollbar_top(pane_id, row, grab);
    }

    fn drag_scrollbar(&mut self, row: u16) {
        let Some(drag) = self.scrollbar_drag.as_ref() else {
            return;
        };
        self.set_scrollbar_top(drag.pane, row, drag.grab);
    }

    fn set_scrollbar_top(&mut self, pane_id: PaneId, row: u16, grab: u16) {
        let Some(rect) = self.state.ui.pane_rects.get(&pane_id).copied() else {
            return;
        };
        let Some(pane) = self.state.panes.get(&pane_id) else {
            return;
        };
        let Some(document) = pane.active.and_then(|id| self.state.documents.get(&id)) else {
            return;
        };
        let Some(scrollbar) = scrollbar_geometry(&self.state, pane, document, rect) else {
            return;
        };
        let travel = scrollbar.track.height.saturating_sub(scrollbar.thumb_len);
        let offset = row
            .saturating_sub(scrollbar.track.y)
            .saturating_sub(grab)
            .min(travel);
        let top = if travel == 0 {
            0
        } else {
            usize::from(offset) * scrollbar.max_top / usize::from(travel)
        };
        if let Some(pane) = self.state.panes.get_mut(&pane_id) {
            pane.scroll_top = top;
            pane.scroll_row = 0;
        }
        self.dirty = true;
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
            let tab_width = document.indent.tab_width.max(1);
            let last = (pane.scroll_top + rows).min(document.line_count());
            let max_width = (pane.scroll_top..last)
                .filter(|line| !document.is_hidden(*line))
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
        pane.scroll_row = 0;
        pane.scroll_left = left;
        self.state.focus = Focus::Pane(pane_id);
        self.state.workspaces.active_mut().focus = pane_id;
    }

    /// Document tabs inside a pane border. Rendering and hit-testing share
    /// `ui::pane::document_tabs` and `tab_strip_geometry`.
    fn document_tab_hit(&self, column: u16, row: u16) -> Option<(PaneId, DocumentTabHit)> {
        if !self.state.config.appearance.show_document_tabs {
            return None;
        }
        for pane_id in self.state.panes.keys() {
            let Some((tabs, inner, geometry)) = self.document_tab_geometry(*pane_id) else {
                continue;
            };
            if row != inner.y || column < inner.x || column >= inner.x + inner.width {
                continue;
            }
            if geometry.overflow {
                if column == inner.x {
                    return Some((*pane_id, DocumentTabHit::Scroll(-1)));
                }
                if column == inner.x + inner.width - 1 {
                    return Some((*pane_id, DocumentTabHit::Scroll(1)));
                }
            }
            if !contains(geometry.content, column, row) {
                continue;
            }
            let x = column
                .saturating_sub(geometry.content.x)
                .saturating_add(geometry.scroll);
            let index = tabs.partition_point(|tab| tab.start <= x);
            if let Some(tab) = index.checked_sub(1).and_then(|index| tabs.get(index))
                && x < tab.close_start().saturating_add(1)
            {
                let hit = if x >= tab.close_start() {
                    DocumentTabHit::Close(tab.id)
                } else {
                    DocumentTabHit::Activate(tab.id)
                };
                return Some((*pane_id, hit));
            }
        }
        None
    }

    fn document_tab_geometry(
        &self,
        pane_id: PaneId,
    ) -> Option<(Vec<DocumentTab>, Rect, crate::ui::pane::TabStripGeometry)> {
        let pane = self.state.panes.get(&pane_id)?;
        let rect = self.state.ui.pane_rects.get(&pane_id).copied()?;
        let inner = Block::default().borders(Borders::ALL).inner(rect);
        if inner.height < 3 || inner.width == 0 {
            return None;
        }
        let tab_width = self.state.config.editor.tab_size.max(1);
        let tabs = document_tabs(&self.state, pane, tab_width);
        let total = tabs
            .last()
            .map(|tab| tab.start.saturating_add(tab.width))
            .unwrap_or(0);
        let geometry = tab_strip_geometry(inner, total, pane.tab_scroll);
        Some((tabs, inner, geometry))
    }

    fn scroll_document_tabs(&mut self, pane_id: PaneId, direction: isize) {
        let Some((tabs, _, geometry)) = self.document_tab_geometry(pane_id) else {
            return;
        };
        if !geometry.overflow {
            return;
        }
        let target = if direction < 0 {
            tabs.iter()
                .filter(|tab| tab.start < geometry.scroll)
                .map(|tab| tab.start)
                .max()
                .unwrap_or(0)
        } else if direction > 0 {
            tabs.iter()
                .find(|tab| tab.start > geometry.scroll)
                .map(|tab| tab.start)
                .unwrap_or(geometry.max_scroll)
        } else {
            return;
        };
        let target = target.min(geometry.max_scroll);
        if let Some(pane) = self.state.panes.get_mut(&pane_id) {
            pane.tab_scroll = target;
            pane.tab_follow = false;
        }
        self.state.focus = Focus::Pane(pane_id);
        self.state.workspaces.active_mut().focus = pane_id;
        self.dirty = true;
    }

    /// Keep the active document's tab visible after activation or a close.
    pub(crate) fn reveal_document_tab(&mut self, pane_id: PaneId, id: DocumentId) {
        if let Some(pane) = self.state.panes.get_mut(&pane_id) {
            pane.tab_follow = true;
        }
        let Some((tabs, _, geometry)) = self.document_tab_geometry(pane_id) else {
            return;
        };
        if !geometry.overflow {
            return;
        }
        let Some(tab) = tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let mut scroll = geometry.scroll;
        if tab.start < scroll {
            scroll = tab.start;
        } else if tab.start.saturating_add(tab.width) > scroll + geometry.content.width {
            scroll = tab
                .start
                .saturating_add(tab.width)
                .saturating_sub(geometry.content.width);
        }
        let scroll = scroll.min(geometry.max_scroll);
        if let Some(pane) = self.state.panes.get_mut(&pane_id) {
            pane.tab_scroll = scroll;
        }
        self.dirty = true;
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

    fn resize_to(&mut self, drag: Drag, column: u16, row: u16) {
        let position = match drag.axis {
            SplitAxis::LeftRight => column,
            SplitAxis::TopBottom => row,
        };
        let index = drag.index;
        if index + 1 >= drag.cells.len() || index + 1 >= drag.mins.len() {
            return;
        }
        // The stored cells and pointer stay at the drag origin, so every event
        // applies its whole displacement once. Folding the result back in would
        // re-apply each delta to the already-moved border and make it run away.
        let delta = i64::from(position) - i64::from(drag.start);
        let total = i64::from(drag.cells[index]) + i64::from(drag.cells[index + 1]);
        let min_left = i64::from(drag.mins[index]);
        let min_right = i64::from(drag.mins[index + 1]);
        let upper = (total - min_right).max(min_left);
        let left = (i64::from(drag.cells[index]) + delta).clamp(min_left, upper);
        let mut cells = drag.cells.clone();
        cells[index] = left as u16;
        cells[index + 1] = (total - left) as u16;

        self.state
            .workspaces
            .active_mut()
            .layout
            .resize(&drag.path, index, &cells);
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

/// Body of the terminal panel: the area inside its one-cell border.
fn shell_inner(area: Rect) -> Option<Rect> {
    if area.width <= 2 || area.height <= 2 {
        return None;
    }
    Some(Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width - 2,
        area.height - 2,
    ))
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
