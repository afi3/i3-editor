#!/usr/bin/env python3
"""Record docs/demo.gif by driving the real binary in a PTY.

Dev-only dependencies: `pip install pyte pillow`. Run from the repository root:

    cargo build --release
    python3 scripts/make_demo.py
"""

import codecs
import fcntl
import os
import pty
import select
import signal
import struct
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/i3-editor"
OUT = ROOT / "docs" / "demo.gif"
COLS, ROWS = 100, 30
FPS = 8
FONT_PATH = "/System/Library/Fonts/Menlo.ttc"
FONT_SIZE = 13

NAMED = {
    "black": (0, 0, 0),
    "red": (205, 49, 49),
    "green": (13, 188, 121),
    "brown": (229, 229, 16),
    "yellow": (229, 229, 16),
    "blue": (36, 114, 200),
    "magenta": (188, 63, 188),
    "cyan": (17, 168, 205),
    "white": (229, 229, 229),
    "brightblack": (102, 102, 102),
    "brightred": (241, 76, 76),
    "brightgreen": (35, 209, 139),
    "brightyellow": (245, 245, 67),
    "brightblue": (59, 142, 234),
    "brightmagenta": (214, 112, 214),
    "brightcyan": (41, 184, 219),
    "brightwhite": (255, 255, 255),
}
DEFAULT_BG = (16, 16, 20)
DEFAULT_FG = (230, 230, 230)

SAMPLE = """\
//! Demo file for the README recording.

use std::collections::HashMap;

const MAX_TABS: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub id: u32,
    pub title: String,
    pub dirty: bool,
}

impl Pane {
    /// Open a document in this pane.
    pub fn open(&mut self, title: &str) -> bool {
        if title.is_empty() {
            return false;
        }
        self.title = title.to_string();
        self.dirty = true;
        true
    }
}

fn main() {
    let mut panes: HashMap<u32, Pane> = HashMap::new();
    panes.insert(1, Pane { id: 1, title: "main.rs".into(), dirty: false });
    let count = panes.len();
    println!("{count} panes, up to {MAX_TABS} tabs");
}
"""


def colour(name, default):
    if not name or name == "default":
        return default
    if name in NAMED:
        return NAMED[name]
    if len(name) == 6:
        try:
            return tuple(int(name[i : i + 2], 16) for i in (0, 2, 4))
        except ValueError:
            return default
    return default


def make_workspace(base: Path) -> tuple[Path, Path]:
    (base / "src").mkdir(parents=True)
    (base / "docs").mkdir()
    (base / "src" / "main.rs").write_text(SAMPLE)
    (base / "Cargo.toml").write_text(
        '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n'
    )
    (base / "README.md").write_text("# demo\n\nA workspace used for the README recording.\n")
    (base / "docs" / "notes.md").write_text("## notes\n\n- panes\n- tabs\n- themes\n")

    config = base.parent / "cfg" / "i3-editor"
    themes = config / "themes"
    themes.mkdir(parents=True, exist_ok=True)
    (config / "config.toml").write_text(
        '[appearance]\ntheme = "demo"\n\n[terminal]\nheight = 9\n\n[explorer]\nwidth = 22\n'
    )
    (themes / "demo.toml").write_text(
        'base = "dark"\n\n'
        'background = "#101014"\n'
        'foreground = "#e6e6e6"\n'
        'current_line = "#18181f"\n'
        'border_active = "#7aa2f7"\n'
        'explorer_dir = "#7aa2f7"\n\n'
        "[syntax]\n"
        'keyword = "#c678dd"\n'
        'string = "#98c379"\n'
        'comment = "#5c6370"\n'
        'number = "#d19a66"\n'
        'function = "#61afef"\n'
        'type_name = "#e5c07b"\n'
        'constant = "#56b6c2"\n'
        'operator = "#abb2bf"\n'
        'punctuation = "#8b93a1"\n'
    )
    return base, config.parent


def schedule():
    keys = []
    t = 0.0

    def at(delay, data):
        nonlocal t
        t += delay
        keys.append((t, data))

    at(0.6, b"\x1bOR")                 # F3 split left/right
    at(1.2, b"\x1bOP")                 # F1 palette
    at(0.4, b"toggle_term")            # unique prefix of toggle_terminal
    at(0.3, b"\r")
    at(1.0, b"printf 'terminal panel ready\\n'\r")
    at(1.8, b"\x1bOP")
    at(0.4, b"next_theme")
    at(0.3, b"\r")
    at(1.4, b"\x1bOP")
    at(0.4, b"next_theme")
    at(0.3, b"\r")
    at(1.6, b"\x1b[<0;30;6M\x1b[<0;30;6m")   # click into the left pane
    at(0.5, b"// edited live\r")
    at(0.8, b"\x11")                  # Ctrl+Q, dirty document
    at(1.2, b"\x1b[B")                # choose Discard all and quit
    at(0.4, b"\r")
    return keys


def capture(workdir: Path, cfg_root: Path, keys):
    pid, fd = pty.fork()
    if pid == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        env = dict(os.environ)
        env.pop("NO_COLOR", None)
        env["TERM"] = "xterm-256color"
        env["XDG_CONFIG_HOME"] = str(cfg_root)
        target = str(workdir / "src" / "main.rs")
        os.execve(str(BIN), ["i3-editor", target], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    events = []
    answered = False
    queue = list(keys)
    start = time.monotonic()
    deadline = start + (queue[-1][0] if queue else 1.0) + 1.5
    while time.monotonic() < deadline:
        now = time.monotonic() - start
        while queue and queue[0][0] <= now:
            try:
                os.write(fd, queue.pop(0)[1])
            except OSError:
                queue.clear()
        try:
            ready, _, _ = select.select([fd], [], [], 0.01)
        except OSError:
            break
        if ready:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            events.append((time.monotonic() - start, data))
            if not answered and b"\x1b[c" in data:
                os.write(fd, b"\x1b[?1;2c\x1b[?0u")
                answered = True
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    try:
        os.waitpid(pid, 0)
    except ChildProcessError:
        pass
    return events


def main():
    if not BIN.is_file():
        sys.exit(f"missing {BIN}; run `cargo build --release` first")

    temp = Path(tempfile.mkdtemp(prefix="i3-demo-"))
    workdir, cfg_root = make_workspace(temp / "work")
    events = capture(workdir, cfg_root, schedule())

    font = ImageFont.truetype(FONT_PATH, FONT_SIZE)
    advance = max(6, round(font.getlength("M")))
    line_height = FONT_SIZE + 4
    width = COLS * advance
    height = ROWS * line_height

    screen = pyte.Screen(COLS, ROWS)
    stream = pyte.Stream(screen)
    decoder = codecs.getincrementaldecoder("utf-8")("replace")

    duration = (events[-1][0] if events else 1.0) + 0.4
    total = max(1, int(duration * FPS))
    frames = []
    index = 0
    for frame in range(total):
        cutoff = (frame + 1) / FPS
        while index < len(events) and events[index][0] <= cutoff:
            stream.feed(decoder.decode(events[index][1]))
            index += 1
        frames.append(render(screen, font, advance, line_height, width, height))

    OUT.parent.mkdir(parents=True, exist_ok=True)
    frames[0].save(
        OUT,
        save_all=True,
        append_images=frames[1:],
        duration=int(1000 / FPS),
        loop=0,
        optimize=True,
    )
    print(f"wrote {OUT} ({len(frames)} frames, {width}x{height})")


def render(screen, font, advance, line_height, width, height):
    image = Image.new("RGB", (width, height), DEFAULT_BG)
    draw = ImageDraw.Draw(image)
    for row in range(ROWS):
        runs = []
        for col in range(COLS):
            cell = screen.buffer[row][col]
            fg = colour(cell.fg, DEFAULT_FG)
            bg = colour(cell.bg, DEFAULT_BG)
            if runs and runs[-1][0] == (fg, bg):
                runs[-1][1].append(cell.data or " ")
            else:
                runs.append(((fg, bg), [cell.data or " "]))
        for index, ((fg, bg), chars) in enumerate(runs):
            start = sum(len(c) for _, c in runs[:index]) * advance
            text = "".join(chars)
            y = row * line_height
            if bg != DEFAULT_BG:
                draw.rectangle(
                    (start, y, start + len(text) * advance, y + line_height), fill=bg
                )
            draw.text((start, y), text, font=font, fill=fg, spacing=0)

    cursor = screen.cursor
    if not cursor.hidden and 0 <= cursor.y < ROWS and 0 <= cursor.x < COLS:
        cell = screen.buffer[cursor.y][cursor.x]
        fg = colour(cell.bg, DEFAULT_BG)
        bg = colour(cell.fg, DEFAULT_FG)
        x = cursor.x * advance
        y = cursor.y * line_height
        draw.rectangle((x, y, x + advance, y + line_height), fill=fg)
        draw.text((x, y), cell.data or " ", font=font, fill=bg)
    return image


if __name__ == "__main__":
    main()
