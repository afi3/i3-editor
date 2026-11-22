//! Folding commands.

use crate::error::Result;

use super::App;

impl App {
    /// Fold the block starting at the cursor line, or open the fold covering it.
    pub(crate) fn toggle_fold(&mut self) -> Result<()> {
        let message = {
            let Some((pane, document)) = self.state.pane_and_document() else {
                return Ok(());
            };
            let line = document.line_of_char(pane.cursor);
            match document.toggle_fold(line) {
                Some(0) => "unfolded".to_string(),
                Some(hidden) => {
                    pane.cursor = document.char_of_line(line);
                    pane.anchor = None;
                    pane.target_col = None;
                    format!("folded {hidden} line(s)")
                }
                None => "nothing to fold on this line".to_string(),
            }
        };
        self.notify(message);
        self.ensure_cursor_visible();
        Ok(())
    }
}
