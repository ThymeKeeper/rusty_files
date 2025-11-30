# Rusty Files

A terminal-based file manager written in Rust. Fast, keyboard-driven, and refreshingly free of mouse dependency.

## Features

- **Single-pane tree navigation** - Browse your filesystem with a clean, hierarchical view
- **Multi-file selection** - Select files using Shift+arrows, Ctrl+click, or click-and-drag
- **Standard file operations** - Copy, cut, paste, delete, rename with familiar keyboard shortcuts
- **Sudo support** - Seamlessly handles operations on protected files with password prompts
- **Trash system with undo** - Delete files safely to trash, undo mistakes with Ctrl+Z
- **File opening** - Launch files with system default applications
- **Smart rename** - Full text editing with cursor positioning, selection, and system clipboard integration
- **Status bar** - Real-time feedback on file counts and selection sizes
- **Performance-conscious** - Minimal resource usage, instant response times

## Installation

### From Source

Requirements:
- Rust 1.70 or later
- A terminal emulator (most likely already installed)

```bash
git clone https://github.com/yourusername/rusty_files.git
cd rusty_files
cargo build --release
```

The binary will be available at `target/release/rusty_files`.

Optionally, install system-wide:
```bash
cargo install --path .
```

## Usage

Launch the application:
```bash
rusty_files
```

Or navigate from a specific directory:
```bash
cd /your/directory
rusty_files
```

### Keyboard Shortcuts

#### Navigation
| Key | Action |
|-----|--------|
| `↑/↓` | Move cursor up/down |
| `←` | Go to parent directory |
| `→` or `Enter` | Enter directory / Open file |
| `Shift+↑/↓` | Extend selection |

#### File Operations
| Key | Action |
|-----|--------|
| `Ctrl+C` | Copy selected files |
| `Ctrl+X` | Cut selected files |
| `Ctrl+V` | Paste files |
| `Ctrl+N` | Create new file or directory |
| `Ctrl+R` | Rename file (with full text editing) |
| `Delete` or `Ctrl+D` | Delete selected files (moves to trash) |
| `Ctrl+Z` | Undo last operation |
| `Ctrl+Space` | Toggle selection on current item |

#### Selection
| Key | Action |
|-----|--------|
| `Ctrl+Click` | Toggle individual file selection |
| `Click+Drag` | Select multiple files |
| `Shift+Click` | Select range (terminal support varies) |

#### Rename Mode
When renaming a file (`Ctrl+R`), additional shortcuts become available:

| Key | Action |
|-----|--------|
| `←/→` | Move cursor |
| `Shift+←/→` | Extend selection |
| `Home/End` | Jump to start/end |
| `Shift+Home/End` | Select to start/end |
| `Ctrl+A` | Select all |
| `Ctrl+C/V/X` | Copy/paste/cut text (uses system clipboard) |
| `Backspace/Delete` | Delete character or selection |
| `Enter` | Confirm rename |
| `Esc` | Cancel rename |

#### Application
| Key | Action |
|-----|--------|
| `Ctrl+Q` | Quit application |

## Building From Source

### Dependencies

Rusty Files uses the following Rust crates:
- `ratatui` (0.29) - Terminal UI framework
- `crossterm` (0.28) - Cross-platform terminal manipulation
- `arboard` (3.4) - System clipboard integration

All dependencies are automatically handled by Cargo.

### Compilation

Debug build (faster compilation, slower runtime):
```bash
cargo build
```

Release build (optimized):
```bash
cargo build --release
```

### Platform Support

Tested on:
- Linux (primary development platform)
- macOS (expected to work, uses `xdg-open` equivalent)
- Windows (expected to work, uses `cmd /c start`)

File opening and sudo operations are platform-aware but have received more extensive testing on Linux.

## Design Philosophy

Rusty Files aims to be:
- **Fast** - Because life is too short for spinning cursors
- **Keyboard-first** - Your fingers shouldn't need to leave home row
- **Forgiving** - Undo support and trash system prevent catastrophic mistakes
- **Unobtrusive** - Small binary, minimal dependencies, no configuration files to maintain

## Technical Details

### Trash System

Deleted files are moved to `~/.local/share/rusty_files/trash` with timestamp prefixes, enabling:
- Safe deletion without permanent data loss
- Undo operations via Ctrl+Z
- Manual recovery if needed (files remain accessible in trash directory)

### Sudo Operations

When operations fail due to insufficient permissions:
1. Password prompt appears automatically
2. Credentials are validated before operations
3. Operations are tracked in undo stack
4. Cached credentials are explicitly cleared to prevent password bypass

### Performance Optimizations

- **Lazy size calculation** - File sizes computed on demand and cached
- **Non-recursive directory sizing** - Prevents lag when selecting large directories
- **Efficient rendering** - Only visible portions of file tree are processed
- **Directory state memory** - Returns to previous position when navigating back

## Known Limitations

- Shift+click selection may not work reliably in all terminal emulators (use click-and-drag instead)
- Directory sizes are not calculated recursively (feature, not bug)
- No configuration file support (everything uses sensible defaults)
- Undo stack is in-memory only (cleared when application exits)

## Contributing

Issues and pull requests are welcome. When reporting bugs, please include:
- Operating system and version
- Terminal emulator being used
- Steps to reproduce the issue
- Expected vs. actual behavior

## Acknowledgments

Built with:
- [Ratatui](https://github.com/ratatui-org/ratatui) - Terminal UI framework
- [Crossterm](https://github.com/crossterm-rs/crossterm) - Cross-platform terminal library
- [Arboard](https://github.com/1Password/arboard) - Clipboard library

---

Made with Rust. Tested with files.
