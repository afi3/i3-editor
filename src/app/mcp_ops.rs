//! MCP menu, status overlay, and event pump.

use crate::input::KeyCombo;
use crate::state::HelpView;

use super::App;

impl App {
    pub(crate) fn open_mcp(&mut self) {
        self.state.mcp_view = Some(HelpView {
            lines: self.mcp_lines(),
            scroll: 0,
        });
        self.state.help = None;
        self.state.completions = None;
        self.state.hover = None;
        self.dirty = true;
    }

    pub(crate) fn handle_mcp_key(&mut self, combo: KeyCombo) -> bool {
        let Some(view) = self.state.mcp_view.as_mut() else {
            return false;
        };
        if super::help::read_only_key(view, combo) {
            self.state.mcp_view = None;
        }
        true
    }

    pub(crate) fn pump_mcp(&mut self) {
        let Some(manager) = self.mcp.as_mut() else {
            return;
        };
        let events = manager.pump();
        if events.is_empty() {
            return;
        }
        for event in events {
            match event {
                crate::mcp::Event::Warning(message) => self.notify(message),
                crate::mcp::Event::Tools { .. }
                | crate::mcp::Event::ToolResult { .. }
                | crate::mcp::Event::Log { .. } => {}
            }
        }
        self.dirty = true;
        if self.state.mcp_view.is_some() {
            let scroll = self.state.mcp_view.as_ref().map_or(0, |view| view.scroll);
            self.state.mcp_view = Some(HelpView {
                lines: self.mcp_lines(),
                scroll,
            });
        }
    }

    fn mcp_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "Esc or q closes this overlay".to_string(),
            String::new(),
            "MCP servers".to_string(),
        ];
        if self.state.config.mcp.servers.is_empty() {
            lines.push("No [mcp.<name>] servers configured.".to_string());
            lines.push(String::new());
            lines.push("Add a table to ~/.config/i3-editor/config.toml.".to_string());
            return lines;
        }
        for (name, config) in &self.state.config.mcp.servers {
            let mut args = config.args.join(" ");
            if !args.is_empty() {
                args.insert(0, ' ');
            }
            lines.push(format!("{name}: {}{args}", config.command));
            if let Some(tools) = self.mcp.as_ref().and_then(|mcp| mcp.tools(name)) {
                lines.extend(
                    tools
                        .iter()
                        .map(|tool| format!("  tool {} — {}", tool.name, tool.description)),
                );
            } else {
                lines.push("  not started (trust/start is the next slice)".to_string());
            }
            if let Some(logs) = self.mcp.as_ref().and_then(|mcp| mcp.logs(name))
                && !logs.is_empty()
            {
                lines.push("  logs:".to_string());
                lines.extend(
                    logs.iter()
                        .rev()
                        .take(3)
                        .rev()
                        .map(|line| format!("    {line}")),
                );
            }
        }
        lines
    }
}
