//! Go-to-symbol jumps.

use crate::error::Result;
use crate::structure::Symbol;

use super::App;

impl App {
    /// Put the cursor on a symbol from the active document, opening a fold that
    /// hides it.
    pub(crate) fn goto_symbol(&mut self, symbol: Symbol) -> Result<()> {
        let Some(id) = self.state.active_document_id() else {
            return Ok(());
        };
        let line = {
            let Some(document) = self.state.documents.get_mut(&id) else {
                return Ok(());
            };
            let line = symbol.line.min(document.line_count().saturating_sub(1));
            document.unfold_line(line);
            let text = document.line_text(line);
            let byte = symbol.column.min(text.len());
            let chars = text
                .char_indices()
                .take_while(|(index, _)| *index < byte)
                .count();
            document.char_of_line(line) + chars
        };
        if let Some(pane) = self
            .state
            .focused_pane_mut()
            .filter(|pane| pane.active == Some(id))
        {
            pane.cursor = line;
            pane.anchor = None;
            pane.target_col = None;
        }
        self.ensure_cursor_visible();
        self.notify(format!("{} (line {})", symbol.name, symbol.line + 1));
        Ok(())
    }
}
