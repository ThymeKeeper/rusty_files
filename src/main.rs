use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind, EnableMouseCapture, DisableMouseCapture},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

#[derive(Clone, Debug)]
struct DirEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
}

#[derive(Clone, Debug)]
struct DirState {
    cursor_index: usize,
    selected_indices: HashSet<usize>,
    scroll_offset: usize,
}

#[derive(Clone, Debug)]
enum ClipboardOp {
    Copy,
    Cut,
}

#[derive(Clone, Debug)]
struct Clipboard {
    items: Vec<PathBuf>,
    operation: ClipboardOp,
}

#[derive(Clone, Debug)]
enum UndoAction {
    Copy {
        copied_files: Vec<PathBuf>,
    },
    Move {
        moved_files: Vec<(PathBuf, PathBuf)>,
    },
    Delete {
        deleted_files: Vec<(PathBuf, PathBuf)>,
    },
    Rename {
        original_path: PathBuf,
        new_path: PathBuf,
    },
}

#[derive(Clone, Debug)]
enum CreationType {
    File,
    Directory,
}

#[derive(Clone, Debug)]
enum UIMode {
    Normal,
    PasswordPrompt {
        prompt: String,
        password: String,
        pending_operation: Box<PendingOperation>,
    },
    StatusMessage {
        message: String,
    },
    ConfirmDelete {
        items: Vec<PathBuf>,
    },
    CreateNew {
        creation_type: Option<CreationType>,
        name: String,
    },
    RenameItem {
        original_path: PathBuf,
        new_name: String,
        cursor_pos: usize,
        selection_start: Option<usize>,
    },
}

#[derive(Clone, Debug)]
enum OperationType {
    Copy,
    Move,
    Delete,
    Undo,
}

#[derive(Clone, Debug)]
struct PendingOperation {
    items: Vec<PathBuf>,
    destination: Option<PathBuf>,
    operation: OperationType,
    undo_action: Option<UndoAction>,
}

#[allow(dead_code)]
struct TreeLine {
    text: String,
    entry_index: Option<usize>,
    is_selected: bool,
    is_cursor: bool,
    is_dir: bool,
    is_current_dir: bool,
}

struct FileExplorer {
    current_dir: PathBuf,
    entries: Vec<DirEntry>,
    cursor_index: usize,
    selected_indices: HashSet<usize>,
    selection_anchor: Option<usize>,
    scroll_offset: usize,
    dir_memory: HashMap<PathBuf, DirState>,
    clipboard: Option<Clipboard>,
    ui_mode: UIMode,
    undo_stack: Vec<UndoAction>,
    trash_dir: PathBuf,
    drag_selection: Option<usize>, // Tracks drag start index when dragging
    size_cache: HashMap<PathBuf, u64>, // Cache for file/directory sizes
    current_item_size: Option<u64>, // Size of item currently under cursor
}

impl FileExplorer {
    fn new() -> io::Result<Self> {
        let current_dir = std::env::current_dir()?;

        let trash_dir = if let Some(home) = std::env::var_os("HOME") {
            PathBuf::from(home).join(".local/share/rusty_files/trash")
        } else {
            PathBuf::from("/tmp/rusty_files_trash")
        };

        fs::create_dir_all(&trash_dir)?;

        let mut explorer = FileExplorer {
            current_dir: current_dir.clone(),
            entries: Vec::new(),
            cursor_index: 0,
            selected_indices: HashSet::new(),
            selection_anchor: None,
            scroll_offset: 0,
            dir_memory: HashMap::new(),
            clipboard: None,
            ui_mode: UIMode::Normal,
            undo_stack: Vec::new(),
            trash_dir,
            drag_selection: None,
            size_cache: HashMap::new(),
            current_item_size: None,
        };
        explorer.load_directory()?;
        Ok(explorer)
    }

    fn load_directory(&mut self) -> io::Result<()> {
        self.entries.clear();

        let mut entries = Vec::new();
        if let Ok(read_dir) = fs::read_dir(&self.current_dir) {
            for entry in read_dir.flatten() {
                if let (Ok(name), Ok(metadata)) = (
                    entry.file_name().into_string(),
                    entry.metadata()
                ) {
                    entries.push(DirEntry {
                        path: entry.path(),
                        name,
                        is_dir: metadata.is_dir(),
                    });
                }
            }
        }

        entries.sort_by(|a, b| {
            match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });

        self.entries = entries;

        if let Some(state) = self.dir_memory.get(&self.current_dir) {
            self.cursor_index = state.cursor_index.min(self.entries.len().saturating_sub(1));
            self.selected_indices = state.selected_indices.clone();
            self.scroll_offset = state.scroll_offset;
        } else {
            self.cursor_index = 0;
            self.selected_indices.clear();
            self.scroll_offset = 0;
        }

        self.selection_anchor = None;

        // Clear size cache for new directory and update current item size
        self.size_cache.clear();
        self.update_current_item_size();

        Ok(())
    }

    fn build_tree_lines(&self) -> Vec<TreeLine> {
        let mut lines = Vec::new();
        let ancestors = self.get_ancestors();

        for (depth, path) in ancestors.iter().enumerate() {
            let indent = "  ".repeat(depth);
            let name = path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("/")
                .to_string();

            let is_current = path == &self.current_dir;
            let marker = if is_current { "▼ " } else { "▶ " };

            lines.push(TreeLine {
                text: format!("{}{}{}", indent, marker, name),
                entry_index: None,
                is_selected: false,
                is_cursor: false,
                is_dir: true,
                is_current_dir: is_current,
            });

            if is_current && !self.entries.is_empty() {
                for (i, entry) in self.entries.iter().enumerate() {
                    let child_indent = "  ".repeat(depth + 1);
                    let icon = if entry.is_dir { "📁 " } else { "📄 " };
                    let marker = if entry.is_dir { "▶ " } else { "" };

                    lines.push(TreeLine {
                        text: format!("{}{}{}{}", child_indent, icon, marker, entry.name),
                        entry_index: Some(i),
                        is_selected: self.selected_indices.contains(&i),
                        is_cursor: i == self.cursor_index,
                        is_dir: entry.is_dir,
                        is_current_dir: false,
                    });
                }
            }
        }

        lines
    }

    fn get_cursor_line_index(&self) -> usize {
        let tree_lines = self.build_tree_lines();
        for (line_idx, line) in tree_lines.iter().enumerate() {
            if line.is_cursor {
                return line_idx;
            }
        }
        0
    }

    fn calculate_scroll_offset(&mut self, visible_height: usize, tree_lines: &[TreeLine]) {
        let scrolloff = 1;

        if visible_height == 0 {
            return;
        }

        let cursor_line_idx = tree_lines.iter()
            .position(|line| line.is_cursor)
            .unwrap_or(0);

        if cursor_line_idx < self.scroll_offset + scrolloff {
            self.scroll_offset = cursor_line_idx.saturating_sub(scrolloff);
        } else if cursor_line_idx >= self.scroll_offset + visible_height - scrolloff {
            self.scroll_offset = cursor_line_idx + scrolloff + 1 - visible_height.min(tree_lines.len());
        }

        self.scroll_offset = self.scroll_offset.min(tree_lines.len().saturating_sub(visible_height));
    }

    fn save_state(&mut self) {
        self.dir_memory.insert(
            self.current_dir.clone(),
            DirState {
                cursor_index: self.cursor_index,
                selected_indices: self.selected_indices.clone(),
                scroll_offset: self.scroll_offset,
            },
        );
    }

    fn move_up(&mut self, shift: bool) {
        if self.cursor_index > 0 {
            if shift {
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_index);
                }
            } else {
                self.selected_indices.clear();
                self.selection_anchor = None;
            }

            self.cursor_index -= 1;

            if shift {
                self.update_selection_range();
            }

            self.save_state();
            self.update_current_item_size();
        }
    }

    fn move_down(&mut self, shift: bool) {
        if self.cursor_index < self.entries.len().saturating_sub(1) {
            if shift {
                if self.selection_anchor.is_none() {
                    self.selection_anchor = Some(self.cursor_index);
                }
            } else {
                self.selected_indices.clear();
                self.selection_anchor = None;
            }

            self.cursor_index += 1;

            if shift {
                self.update_selection_range();
            }

            self.save_state();
            self.update_current_item_size();
        }
    }

    fn update_selection_range(&mut self) {
        if let Some(anchor) = self.selection_anchor {
            self.selected_indices.clear();
            let start = anchor.min(self.cursor_index);
            let end = anchor.max(self.cursor_index);
            for i in start..=end {
                self.selected_indices.insert(i);
            }
        }
    }

    fn toggle_selection(&mut self) {
        if self.selected_indices.contains(&self.cursor_index) {
            self.selected_indices.remove(&self.cursor_index);
        } else {
            self.selected_indices.insert(self.cursor_index);
        }
        self.selection_anchor = None;
        self.save_state();
    }

    fn enter_directory(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.load_directory()?;
            }
        }
        Ok(())
    }

    fn open_file(&mut self, path: &PathBuf) -> io::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
        })?;

        if cfg!(target_os = "windows") {
            Command::new("cmd")
                .args(&["/c", "start", "", path_str])
                .spawn()?;
        } else if cfg!(target_os = "macos") {
            Command::new("open")
                .arg(path_str)
                .spawn()?;
        } else {
            // Linux
            Command::new("xdg-open")
                .arg(path_str)
                .spawn()?;
        }

        Ok(())
    }

    fn open_or_enter(&mut self) -> io::Result<()> {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.load_directory()?;
            } else {
                let path = entry.path.clone();
                let name = entry.name.clone();
                if let Err(e) = self.open_file(&path) {
                    self.show_status(format!("Failed to open file: {}", e));
                } else {
                    self.show_status(format!("Opening '{}'", name));
                }
            }
        }
        Ok(())
    }

    fn go_to_parent(&mut self) -> io::Result<()> {
        if let Some(parent) = self.current_dir.parent() {
            let current_dir_name = self.current_dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            self.current_dir = parent.to_path_buf();
            self.load_directory()?;

            for (i, entry) in self.entries.iter().enumerate() {
                if entry.name == current_dir_name {
                    self.cursor_index = i;
                    self.save_state();
                    break;
                }
            }
        }
        Ok(())
    }

    fn handle_mouse_down(&mut self, row: u16, _col: u16, modifiers: KeyModifiers, area_top: u16) {
        let tree_lines = self.build_tree_lines();
        let clicked_line = (row as usize).saturating_sub(area_top as usize + 1).saturating_add(self.scroll_offset);

        if clicked_line < tree_lines.len() {
            if let Some(entry_index) = tree_lines[clicked_line].entry_index {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    // Ctrl+click: toggle individual item
                    self.cursor_index = entry_index;
                    self.toggle_selection();
                    self.update_current_item_size();
                } else {
                    // Regular click: start drag selection
                    self.cursor_index = entry_index;
                    self.drag_selection = Some(entry_index);
                    self.selected_indices.clear();
                    self.selected_indices.insert(entry_index);
                    self.selection_anchor = Some(entry_index);
                    self.save_state();
                    self.update_current_item_size();
                }
            }
        }
    }

    fn handle_mouse_drag(&mut self, row: u16, _col: u16, area_top: u16) {
        if self.drag_selection.is_none() {
            return;
        }

        let tree_lines = self.build_tree_lines();
        let dragged_line = (row as usize).saturating_sub(area_top as usize + 1).saturating_add(self.scroll_offset);

        if dragged_line < tree_lines.len() {
            if let Some(entry_index) = tree_lines[dragged_line].entry_index {
                self.cursor_index = entry_index;
                self.update_selection_range();
                self.save_state();
                self.update_current_item_size();
            }
        }
    }

    fn handle_mouse_up(&mut self) {
        self.drag_selection = None;
    }

    fn copy_selected(&mut self) {
        let items = self.get_selected_paths();
        if !items.is_empty() {
            self.clipboard = Some(Clipboard {
                items,
                operation: ClipboardOp::Copy,
            });
            self.show_status(format!("Copied {} item(s)", self.clipboard.as_ref().unwrap().items.len()));
        }
    }

    fn cut_selected(&mut self) {
        let items = self.get_selected_paths();
        if !items.is_empty() {
            self.clipboard = Some(Clipboard {
                items,
                operation: ClipboardOp::Cut,
            });
            self.show_status(format!("Cut {} item(s)", self.clipboard.as_ref().unwrap().items.len()));
        }
    }

    fn paste(&mut self) -> io::Result<()> {
        if let Some(clipboard) = &self.clipboard {
            let destination = self.current_dir.clone();
            let items = clipboard.items.clone();
            let is_move = matches!(clipboard.operation, ClipboardOp::Cut);

            match self.perform_file_operation_tracked(&items, &destination, is_move) {
                Ok((count, undo_action)) => {
                    if is_move {
                        self.clipboard = None;
                    }

                    // Extract actual pasted filenames from the undo action
                    let pasted_names: Vec<String> = match &undo_action {
                        UndoAction::Move { moved_files } => {
                            moved_files.iter()
                                .filter_map(|(_, dest)| dest.file_name())
                                .filter_map(|n| n.to_str())
                                .map(|s| s.to_string())
                                .collect()
                        }
                        UndoAction::Copy { copied_files } => {
                            copied_files.iter()
                                .filter_map(|p| p.file_name())
                                .filter_map(|n| n.to_str())
                                .map(|s| s.to_string())
                                .collect()
                        }
                        _ => Vec::new(),
                    };

                    self.undo_stack.push(undo_action);
                    self.show_status(format!("Pasted {} item(s)", count));
                    self.load_directory()?;
                    self.select_items_by_name(&pasted_names);
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    self.ui_mode = UIMode::PasswordPrompt {
                        prompt: "Permission denied. Enter sudo password:".to_string(),
                        password: String::new(),
                        pending_operation: Box::new(PendingOperation {
                            items,
                            destination: Some(destination),
                            operation: if is_move { OperationType::Move } else { OperationType::Copy },
                            undo_action: None,
                        }),
                    };
                }
                Err(e) => {
                    self.show_status(format!("Error: {}", e));
                }
            }
        }
        Ok(())
    }

    fn start_create_new(&mut self) {
        self.ui_mode = UIMode::CreateNew {
            creation_type: None,
            name: String::new(),
        };
    }

    fn create_new_item(&mut self, creation_type: CreationType, name: String) -> io::Result<()> {
        if name.is_empty() {
            self.show_status("Name cannot be empty".to_string());
            return Ok(());
        }

        let new_path = self.current_dir.join(&name);

        if new_path.exists() {
            self.show_status(format!("'{}' already exists", name));
            return Ok(());
        }

        match creation_type {
            CreationType::File => {
                fs::File::create(&new_path)?;
                self.show_status(format!("Created file '{}'", name));
            }
            CreationType::Directory => {
                fs::create_dir(&new_path)?;
                self.show_status(format!("Created directory '{}'", name));
            }
        }

        self.load_directory()?;

        // Select the newly created item
        self.select_items_by_name(&[name]);

        Ok(())
    }

    fn start_rename(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            let original_path = entry.path.clone();
            let current_name = entry.name.clone();

            // Find the last dot to separate name from extension
            // Select filename without extension
            let cursor_pos = if let Some(dot_pos) = current_name.rfind('.') {
                // Only treat as extension if dot is not at start (hidden files)
                if dot_pos > 0 {
                    dot_pos
                } else {
                    current_name.len()
                }
            } else {
                current_name.len()
            };

            self.ui_mode = UIMode::RenameItem {
                original_path,
                new_name: current_name,
                cursor_pos,
                selection_start: Some(0), // Select from start to cursor (filename without extension)
            };
        }
    }

    fn rename_item(&mut self, original_path: PathBuf, new_name: String) -> io::Result<()> {
        if new_name.is_empty() {
            self.show_status("Name cannot be empty".to_string());
            return Ok(());
        }

        let parent = original_path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
        })?;

        let new_path = parent.join(&new_name);

        if new_path == original_path {
            self.show_status("Name unchanged".to_string());
            return Ok(());
        }

        if new_path.exists() {
            self.show_status(format!("'{}' already exists", new_name));
            return Ok(());
        }

        // Try to rename, handle permission errors
        match fs::rename(&original_path, &new_path) {
            Ok(_) => {
                self.show_status(format!("Renamed to '{}'", new_name));

                // Add to undo stack
                self.undo_stack.push(UndoAction::Rename {
                    original_path: original_path.clone(),
                    new_path: new_path.clone(),
                });

                // Clear size cache entry for old path
                self.size_cache.remove(&original_path);

                self.load_directory()?;

                // Select the renamed item
                self.select_items_by_name(&[new_name]);

                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(13) => {
                // Need sudo privileges
                self.ui_mode = UIMode::PasswordPrompt {
                    prompt: format!("Enter sudo password to rename '{}':", original_path.file_name().unwrap_or_default().to_string_lossy()),
                    password: String::new(),
                    pending_operation: Box::new(PendingOperation {
                        items: vec![original_path.clone()],
                        destination: Some(new_path),
                        operation: OperationType::Move, // Rename is essentially a move
                        undo_action: None,
                    }),
                };
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn delete_selected(&mut self) {
        let items = self.get_selected_paths();
        if !items.is_empty() {
            self.ui_mode = UIMode::ConfirmDelete { items };
        }
    }

    fn perform_delete(&mut self, items: &[PathBuf]) -> io::Result<()> {
        let mut count = 0;
        let mut deleted_files = Vec::new();

        for item in items {
            let file_name = item.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name")
            })?;

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let trash_name = format!("{}_{}", timestamp, file_name.to_string_lossy());
            let trash_path = self.trash_dir.join(trash_name);

            fs::rename(item, &trash_path)?;
            deleted_files.push((item.clone(), trash_path));
            count += 1;
        }

        self.undo_stack.push(UndoAction::Delete { deleted_files });
        self.show_status(format!("Deleted {} item(s) (moved to trash)", count));
        self.selected_indices.clear();
        self.selection_anchor = None;
        self.save_state(); // Save cleared selection before loading directory
        self.load_directory()?;
        Ok(())
    }

    fn validate_sudo_password(&self, password: &str) -> io::Result<()> {
        // Use sudo -kSv to clear cache (-k) and validate password (-v) from stdin (-S)
        let mut child = Command::new("sudo")
            .arg("-kS")
            .arg("-v")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", password)?;
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Incorrect sudo password"
            ));
        }

        Ok(())
    }

    fn perform_delete_sudo(&self, items: &[PathBuf], password: &str) -> io::Result<Vec<(PathBuf, PathBuf)>> {
        // Validate password first to avoid cached credentials
        self.validate_sudo_password(password)?;
        let mut deleted_files = Vec::new();

        for item in items {
            let file_name = item.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name")
            })?;

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let trash_name = format!("{}_{}", timestamp, file_name.to_string_lossy());
            let trash_path = self.trash_dir.join(trash_name);

            let item_str = item.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
            })?;
            let trash_path_str = trash_path.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid trash path")
            })?;

            let mut child = Command::new("sudo")
                .arg("-S")
                .arg("mv")
                .arg(item_str)
                .arg(trash_path_str)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;

            if let Some(mut stdin) = child.stdin.take() {
                writeln!(stdin, "{}", password)?;
            }

            let output = child.wait_with_output()?;
            if !output.status.success() {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
            }

            deleted_files.push((item.clone(), trash_path));
        }
        Ok(deleted_files)
    }

    fn perform_rename_sudo(&self, original_path: &PathBuf, new_path: &PathBuf, password: &str) -> io::Result<()> {
        // Validate password first to avoid cached credentials
        self.validate_sudo_password(password)?;

        let original_str = original_path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid original path")
        })?;
        let new_str = new_path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid new path")
        })?;

        let mut child = Command::new("sudo")
            .arg("-S")
            .arg("mv")
            .arg(original_str)
            .arg(new_str)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", password)?;
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            let error_msg = String::from_utf8_lossy(&output.stderr);
            return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
        }

        Ok(())
    }

    fn perform_undo_sudo(&self, action: &UndoAction, password: &str) -> io::Result<usize> {
        // Validate password first to avoid cached credentials
        self.validate_sudo_password(password)?;

        let mut count = 0;
        match action {
            UndoAction::Copy { copied_files } => {
                for file in copied_files {
                    if file.exists() {
                        let file_str = file.to_str().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                        })?;

                        let command = if file.is_dir() { "rm" } else { "rm" };
                        let args = if file.is_dir() { vec!["-rf", file_str] } else { vec![file_str] };

                        let mut child = Command::new("sudo")
                            .arg("-S")
                            .arg(command)
                            .args(&args)
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()?;

                        if let Some(mut stdin) = child.stdin.take() {
                            writeln!(stdin, "{}", password)?;
                        }

                        let output = child.wait_with_output()?;
                        if !output.status.success() {
                            let error_msg = String::from_utf8_lossy(&output.stderr);
                            return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
                        }

                        count += 1;
                    }
                }
            }
            UndoAction::Move { moved_files } => {
                for (original, moved_to) in moved_files {
                    if moved_to.exists() {
                        let moved_to_str = moved_to.to_str().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                        })?;
                        let original_str = original.to_str().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                        })?;

                        let mut child = Command::new("sudo")
                            .arg("-S")
                            .arg("mv")
                            .arg(moved_to_str)
                            .arg(original_str)
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()?;

                        if let Some(mut stdin) = child.stdin.take() {
                            writeln!(stdin, "{}", password)?;
                        }

                        let output = child.wait_with_output()?;
                        if !output.status.success() {
                            let error_msg = String::from_utf8_lossy(&output.stderr);
                            return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
                        }

                        count += 1;
                    }
                }
            }
            UndoAction::Delete { deleted_files } => {
                for (original, trash_path) in deleted_files {
                    if trash_path.exists() {
                        let trash_path_str = trash_path.to_str().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                        })?;
                        let original_str = original.to_str().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                        })?;

                        let mut child = Command::new("sudo")
                            .arg("-S")
                            .arg("mv")
                            .arg(trash_path_str)
                            .arg(original_str)
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()?;

                        if let Some(mut stdin) = child.stdin.take() {
                            writeln!(stdin, "{}", password)?;
                        }

                        let output = child.wait_with_output()?;
                        if !output.status.success() {
                            let error_msg = String::from_utf8_lossy(&output.stderr);
                            return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
                        }

                        count += 1;
                    }
                }
            }
            UndoAction::Rename { original_path, new_path } => {
                if new_path.exists() {
                    let new_path_str = new_path.to_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Invalid new path")
                    })?;
                    let original_str = original_path.to_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Invalid original path")
                    })?;

                    let mut child = Command::new("sudo")
                        .arg("-S")
                        .arg("mv")
                        .arg(new_path_str)
                        .arg(original_str)
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()?;

                    if let Some(mut stdin) = child.stdin.take() {
                        writeln!(stdin, "{}", password)?;
                    }

                    let output = child.wait_with_output()?;
                    if !output.status.success() {
                        let error_msg = String::from_utf8_lossy(&output.stderr);
                        return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
                    }

                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn select_items_by_name(&mut self, names: &[String]) {
        self.selected_indices.clear();
        for (i, entry) in self.entries.iter().enumerate() {
            if names.contains(&entry.name) {
                self.selected_indices.insert(i);
            }
        }
        if let Some(&first_idx) = self.selected_indices.iter().next() {
            self.cursor_index = first_idx;
        }
        self.save_state();
    }

    fn undo(&mut self) -> io::Result<()> {
        if let Some(action) = self.undo_stack.pop() {
            let action_clone = action.clone();
            let result: io::Result<()> = match action {
                UndoAction::Copy { copied_files } => {
                    let mut count = 0;
                    for file in &copied_files {
                        if file.exists() {
                            if file.is_dir() {
                                if let Err(e) = fs::remove_dir_all(file) {
                                    return self.handle_undo_error(e, action_clone);
                                }
                            } else {
                                if let Err(e) = fs::remove_file(file) {
                                    return self.handle_undo_error(e, action_clone);
                                }
                            }
                            count += 1;
                        }
                    }
                    self.show_status(format!("Undone copy: removed {} item(s)", count));
                    Ok(())
                }
                UndoAction::Move { moved_files } => {
                    let mut count = 0;
                    for (original, moved_to) in &moved_files {
                        if moved_to.exists() {
                            if let Err(e) = fs::rename(moved_to, original) {
                                return self.handle_undo_error(e, action_clone);
                            }
                            count += 1;
                        }
                    }
                    self.show_status(format!("Undone move: restored {} item(s)", count));
                    Ok(())
                }
                UndoAction::Delete { deleted_files } => {
                    let mut count = 0;
                    for (original, trash_path) in &deleted_files {
                        if trash_path.exists() {
                            if let Err(e) = fs::rename(trash_path, original) {
                                return self.handle_undo_error(e, action_clone);
                            }
                            count += 1;
                        }
                    }
                    self.show_status(format!("Undone delete: restored {} item(s)", count));
                    Ok(())
                }
                UndoAction::Rename { original_path, new_path } => {
                    // Rename back from new_path to original_path
                    if new_path.exists() {
                        if let Err(e) = fs::rename(&new_path, &original_path) {
                            return self.handle_undo_error(e, action_clone);
                        }
                        let original_name = original_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        self.show_status(format!("Undone rename: restored to '{}'", original_name));
                    } else {
                        self.show_status("Cannot undo rename: file not found".to_string());
                    }
                    Ok(())
                }
            };

            match result {
                Ok(_) => {
                    if let Err(e) = self.load_directory() {
                        self.show_status(format!("Warning: {}", e));
                    }
                }
                Err(e) => {
                    return self.handle_undo_error(e, action_clone);
                }
            }
        } else {
            self.show_status("Nothing to undo".to_string());
        }
        Ok(())
    }

    fn handle_undo_error(&mut self, e: io::Error, action: UndoAction) -> io::Result<()> {
        // Check if this is a permission error
        let is_permission_error = e.kind() == io::ErrorKind::PermissionDenied
            || e.raw_os_error() == Some(13);

        if is_permission_error {
            // Push the action back onto the stack
            self.undo_stack.push(action.clone());
            // Prompt for sudo password
            self.ui_mode = UIMode::PasswordPrompt {
                prompt: "Permission denied. Enter sudo password:".to_string(),
                password: String::new(),
                pending_operation: Box::new(PendingOperation {
                    items: Vec::new(),
                    destination: None,
                    operation: OperationType::Undo,
                    undo_action: Some(action),
                }),
            };
            // Always return Ok for permission errors
            Ok(())
        } else {
            // For non-permission errors, show as status and don't crash
            self.show_status(format!("Undo error: {}", e));
            Ok(())
        }
    }

    fn get_unique_path(&self, dest_path: &PathBuf) -> PathBuf {
        if !dest_path.exists() {
            return dest_path.clone();
        }

        let parent = dest_path.parent().unwrap();
        let file_name = dest_path.file_name().unwrap().to_str().unwrap();

        // Split into name and extension
        let (name, ext) = if let Some(dot_pos) = file_name.rfind('.') {
            let name = &file_name[..dot_pos];
            let ext = &file_name[dot_pos..]; // includes the dot
            (name, ext)
        } else {
            (file_name, "")
        };

        // Try name (1).ext, name (2).ext, etc.
        let mut counter = 1;
        loop {
            let new_name = format!("{} ({}){}", name, counter, ext);
            let new_path = parent.join(new_name);
            if !new_path.exists() {
                return new_path;
            }
            counter += 1;
        }
    }

    fn perform_file_operation_tracked(&self, items: &[PathBuf], destination: &PathBuf, is_move: bool) -> io::Result<(usize, UndoAction)> {
        let mut count = 0;
        let mut tracked_operations = Vec::new();
        let mut copied_files = Vec::new();

        for item in items {
            let file_name = item.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name")
            })?;
            let initial_dest_path = destination.join(file_name);
            // Get a unique path to avoid conflicts
            let dest_path = self.get_unique_path(&initial_dest_path);

            if is_move {
                fs::rename(item, &dest_path)?;
                tracked_operations.push((item.clone(), dest_path.clone()));
            } else {
                if item.is_dir() {
                    self.copy_dir_recursive(item, &dest_path)?;
                } else {
                    fs::copy(item, &dest_path)?;
                }
                copied_files.push(dest_path.clone());
            }
            count += 1;
        }

        let undo_action = if is_move {
            UndoAction::Move {
                moved_files: tracked_operations,
            }
        } else {
            UndoAction::Copy { copied_files }
        };

        Ok((count, undo_action))
    }

    fn copy_dir_recursive(&self, src: &PathBuf, dst: &PathBuf) -> io::Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if file_type.is_dir() {
                self.copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    fn perform_file_operation_sudo(&self, items: &[PathBuf], destination: &PathBuf, is_move: bool, password: &str) -> io::Result<usize> {
        // Validate password first to avoid cached credentials
        self.validate_sudo_password(password)?;

        let mut count = 0;
        for item in items {
            let file_name = item.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name")
            })?;
            let initial_dest_path = destination.join(file_name);
            // Get a unique path to avoid conflicts
            let dest_path = self.get_unique_path(&initial_dest_path);

            let command = if is_move { "mv" } else { "cp" };
            let mut args = vec!["-r"];
            args.push(item.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
            })?);
            args.push(dest_path.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
            })?);

            let mut child = Command::new("sudo")
                .arg("-S")
                .arg(command)
                .args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;

            if let Some(mut stdin) = child.stdin.take() {
                writeln!(stdin, "{}", password)?;
            }

            let output = child.wait_with_output()?;
            if !output.status.success() {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
            }

            count += 1;
        }
        Ok(count)
    }

    fn get_selected_paths(&self) -> Vec<PathBuf> {
        let indices = if self.selected_indices.is_empty() {
            vec![self.cursor_index]
        } else {
            self.selected_indices.iter().cloned().collect()
        };

        indices.iter()
            .filter_map(|&i| self.entries.get(i))
            .map(|entry| entry.path.clone())
            .collect()
    }

    fn format_file_size(size: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if size >= GB {
            format!("{:.2} GB", size as f64 / GB as f64)
        } else if size >= MB {
            format!("{:.2} MB", size as f64 / MB as f64)
        } else if size >= KB {
            format!("{:.2} KB", size as f64 / KB as f64)
        } else {
            format!("{} B", size)
        }
    }

    fn get_file_size(path: &PathBuf) -> u64 {
        if let Ok(metadata) = fs::metadata(path) {
            if metadata.is_file() {
                return metadata.len();
            }
            // For directories, return 0 (don't recurse to avoid performance issues)
        }
        0
    }

    fn get_selected_total_size(&self) -> u64 {
        self.selected_indices
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .map(|entry| Self::get_file_size(&entry.path))
            .sum()
    }

    fn update_current_item_size(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_index) {
            let path = &entry.path;
            if let Some(&cached_size) = self.size_cache.get(path) {
                self.current_item_size = Some(cached_size);
            } else {
                let size = Self::get_file_size(path);
                self.size_cache.insert(path.clone(), size);
                self.current_item_size = Some(size);
            }
        } else {
            self.current_item_size = None;
        }
    }

    fn show_status(&mut self, message: String) {
        self.ui_mode = UIMode::StatusMessage { message };
    }

    fn clear_status(&mut self) {
        if matches!(self.ui_mode, UIMode::StatusMessage { .. }) {
            self.ui_mode = UIMode::Normal;
        }
    }

    fn get_ancestors(&self) -> Vec<PathBuf> {
        let mut ancestors = Vec::new();
        let mut current = self.current_dir.clone();

        ancestors.push(current.clone());

        while let Some(parent) = current.parent() {
            if parent == current {
                break;
            }
            current = parent.to_path_buf();
            ancestors.insert(0, current.clone());
        }

        ancestors
    }
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    mut explorer: FileExplorer,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| {
            let area = f.area();

            let chunks = match &explorer.ui_mode {
                UIMode::Normal => Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .split(area)
                    .to_vec(),
                _ => Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(3),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .split(area)
                    .to_vec(),
            };

            let main_area = chunks[0];
            let status_bar_area = chunks[1];
            let visible_height = main_area.height.saturating_sub(2) as usize;

            let tree_lines = explorer.build_tree_lines();
            explorer.calculate_scroll_offset(visible_height, &tree_lines);

            let tree_items: Vec<ListItem> = tree_lines
                .iter()
                .map(|tree_line| {
                    let style = if tree_line.is_cursor && tree_line.is_selected {
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::Blue)
                            .add_modifier(Modifier::BOLD)
                    } else if tree_line.is_cursor {
                        Style::default()
                            .fg(Color::White)
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else if tree_line.is_selected {
                        Style::default()
                            .fg(Color::LightCyan)
                            .bg(Color::DarkGray)
                    } else if tree_line.is_current_dir {
                        Style::default()
                            .fg(Color::Blue)
                    } else if tree_line.is_dir {
                        Style::default()
                            .fg(Color::DarkGray)
                    } else {
                        Style::default()
                            .fg(Color::Gray)
                    };

                    ListItem::new(Line::from(Span::styled(&tree_line.text, style)))
                })
                .collect();

            let current_dir_str = explorer.current_dir.display().to_string();
            let title_style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD);
            let border_style = Style::default().fg(Color::DarkGray);

            let tree_list = List::new(tree_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(Span::styled(format!("File Explorer: {}", current_dir_str), title_style))
                );

            let cursor_line_idx = explorer.get_cursor_line_index();
            let mut list_state = ListState::default()
                .with_selected(Some(cursor_line_idx))
                .with_offset(explorer.scroll_offset);

            f.render_stateful_widget(tree_list, main_area, &mut list_state);

            // Render status bar
            let total_items = explorer.entries.len();
            let selected_count = explorer.selected_indices.len();
            let status_text = if selected_count > 0 {
                let total_size = explorer.get_selected_total_size();
                let size_str = FileExplorer::format_file_size(total_size);
                format!("{} items | {} selected | {}", total_items, selected_count, size_str)
            } else if let Some(entry) = explorer.entries.get(explorer.cursor_index) {
                if entry.is_dir {
                    format!("{} items | Directory: {}", total_items, entry.name)
                } else {
                    let item_size = explorer.current_item_size.unwrap_or(0);
                    let size_str = FileExplorer::format_file_size(item_size);
                    format!("{} items | File: {} | {}", total_items, entry.name, size_str)
                }
            } else {
                format!("{} items", total_items)
            };

            let status_bar = Paragraph::new(status_text)
                .style(Style::default().fg(Color::Gray).bg(Color::Black))
                .alignment(Alignment::Left);
            f.render_widget(status_bar, status_bar_area);

            if chunks.len() > 2 {
                match &explorer.ui_mode {
                    UIMode::PasswordPrompt { prompt, password, .. } => {
                        let masked_password = "*".repeat(password.len());
                        let text = format!("{}\n{}", prompt, masked_password);
                        let para = Paragraph::new(text)
                            .block(Block::default().borders(Borders::ALL).title("Password Required"))
                            .style(Style::default().fg(Color::Yellow))
                            .wrap(Wrap { trim: false });
                        f.render_widget(para, chunks[2]);
                    }
                    UIMode::StatusMessage { message } => {
                        let para = Paragraph::new(message.as_str())
                            .block(Block::default().borders(Borders::ALL).title("Status"))
                            .style(Style::default().fg(Color::Cyan))
                            .alignment(Alignment::Left);
                        f.render_widget(para, chunks[2]);
                    }
                    UIMode::ConfirmDelete { items } => {
                        let text = format!("Delete {} item(s)? (y/n)", items.len());
                        let para = Paragraph::new(text)
                            .block(Block::default().borders(Borders::ALL).title("Confirm Delete"))
                            .style(Style::default().fg(Color::Red))
                            .alignment(Alignment::Left);
                        f.render_widget(para, chunks[2]);
                    }
                    UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } => {
                        // Build text with cursor and selection highlighting
                        let mut spans = vec![Span::raw("Rename to: ")];

                        // Get selection range if any
                        let sel_range = selection_start.map(|sel_start| {
                            let start = sel_start.min(*cursor_pos);
                            let end = sel_start.max(*cursor_pos);
                            (start, end)
                        });

                        // Render character by character to properly overlay cursor
                        for (i, ch) in new_name.chars().enumerate() {
                            let is_selected = sel_range.map_or(false, |(start, end)| i >= start && i < end);
                            let is_cursor = i == *cursor_pos;

                            let style = if is_cursor && is_selected {
                                // Cursor on selected text - use inverted selection colors
                                Style::default().bg(Color::White).fg(Color::Blue)
                            } else if is_cursor {
                                // Cursor - use reverse video
                                Style::default().bg(Color::Yellow).fg(Color::Black)
                            } else if is_selected {
                                // Selected text
                                Style::default().bg(Color::Blue).fg(Color::White)
                            } else {
                                // Normal text
                                Style::default()
                            };

                            spans.push(Span::styled(ch.to_string(), style));
                        }

                        // If cursor is at the end (past all characters), show a block cursor
                        if *cursor_pos >= new_name.len() {
                            spans.push(Span::styled("█", Style::default().bg(Color::Yellow).fg(Color::Black)));
                        }

                        let text = Line::from(spans);
                        let para = Paragraph::new(text)
                            .block(Block::default().borders(Borders::ALL).title("Rename"))
                            .style(Style::default().fg(Color::Yellow))
                            .alignment(Alignment::Left);
                        f.render_widget(para, chunks[2]);
                    }
                    UIMode::CreateNew { creation_type, name } => {
                        let text = if creation_type.is_none() {
                            "Create new: (f)ile or (d)irectory?".to_string()
                        } else {
                            let type_str = match creation_type {
                                Some(CreationType::File) => "file",
                                Some(CreationType::Directory) => "directory",
                                None => unreachable!(),
                            };
                            format!("Enter {} name: {}", type_str, name)
                        };
                        let para = Paragraph::new(text)
                            .block(Block::default().borders(Borders::ALL).title("Create New"))
                            .style(Style::default().fg(Color::Green))
                            .alignment(Alignment::Left);
                        f.render_widget(para, chunks[2]);
                    }
                    _ => {}
                }
            }
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Auto-dismiss status messages on any key press and process the key
                    if matches!(explorer.ui_mode, UIMode::StatusMessage { .. }) {
                        explorer.clear_status();
                    }

                    match &explorer.ui_mode.clone() {
                        UIMode::PasswordPrompt { prompt: _, password, pending_operation } => {
                            match key.code {
                                KeyCode::Char(c) => {
                                    if let UIMode::PasswordPrompt { password, .. } = &mut explorer.ui_mode {
                                        password.push(c);
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let UIMode::PasswordPrompt { password, .. } = &mut explorer.ui_mode {
                                        password.pop();
                                    }
                                }
                                KeyCode::Enter => {
                                    let op = pending_operation.clone();
                                    let pwd = password.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    match &op.operation {
                                        OperationType::Copy | OperationType::Move => {
                                            let is_move = matches!(op.operation, OperationType::Move);
                                            if let Some(dest) = &op.destination {
                                                // Check if this is a rename operation (single item, same parent directory)
                                                let is_rename = op.items.len() == 1
                                                    && op.items[0].parent() == dest.parent();

                                                if is_rename {
                                                    // Handle rename with sudo
                                                    let original_path = &op.items[0];
                                                    let new_name = dest.file_name()
                                                        .and_then(|n| n.to_str())
                                                        .unwrap_or("")
                                                        .to_string();

                                                    match explorer.perform_rename_sudo(original_path, dest, &pwd) {
                                                        Ok(_) => {
                                                            explorer.show_status(format!("Renamed to '{}' with sudo", new_name));

                                                            // Add to undo stack
                                                            explorer.undo_stack.push(UndoAction::Rename {
                                                                original_path: original_path.clone(),
                                                                new_path: dest.clone(),
                                                            });

                                                            // Clear size cache entry for old path
                                                            explorer.size_cache.remove(original_path);

                                                            explorer.load_directory()?;
                                                            explorer.select_items_by_name(&[new_name]);
                                                        }
                                                        Err(e) => {
                                                            explorer.show_status(format!("Error: {}", e));
                                                        }
                                                    }
                                                } else {
                                                    // Handle copy/move with sudo
                                                    let pasted_names: Vec<String> = op.items.iter()
                                                        .filter_map(|p| p.file_name())
                                                        .filter_map(|n| n.to_str())
                                                        .map(|s| s.to_string())
                                                        .collect();

                                                    match explorer.perform_file_operation_sudo(&op.items, dest, is_move, &pwd) {
                                                        Ok(count) => {
                                                            if is_move {
                                                                explorer.clipboard = None;
                                                            }
                                                            explorer.show_status(format!("Pasted {} item(s) with sudo", count));
                                                            explorer.load_directory()?;
                                                            explorer.select_items_by_name(&pasted_names);
                                                        }
                                                        Err(e) => {
                                                            explorer.show_status(format!("Error: {}", e));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        OperationType::Delete => {
                                            match explorer.perform_delete_sudo(&op.items, &pwd) {
                                                Ok(deleted_files) => {
                                                    let count = deleted_files.len();
                                                    explorer.undo_stack.push(UndoAction::Delete { deleted_files });
                                                    explorer.show_status(format!("Deleted {} item(s) with sudo (moved to trash)", count));
                                                    explorer.selected_indices.clear();
                                                    explorer.selection_anchor = None;
                                                    explorer.load_directory()?;
                                                }
                                                Err(e) => {
                                                    explorer.show_status(format!("Error: {}", e));
                                                }
                                            }
                                        }
                                        OperationType::Undo => {
                                            if let Some(undo_action) = &op.undo_action {
                                                match explorer.perform_undo_sudo(undo_action, &pwd) {
                                                    Ok(count) => {
                                                        // Pop the action from the stack since we successfully undid it
                                                        explorer.undo_stack.pop();
                                                        let msg = match undo_action {
                                                            UndoAction::Copy { .. } => format!("Undone copy: removed {} item(s) with sudo", count),
                                                            UndoAction::Move { .. } => format!("Undone move: restored {} item(s) with sudo", count),
                                                            UndoAction::Delete { .. } => format!("Undone delete: restored {} item(s) with sudo", count),
                                                            UndoAction::Rename { original_path, .. } => {
                                                                let name = original_path.file_name()
                                                                    .and_then(|n| n.to_str())
                                                                    .unwrap_or("");
                                                                format!("Undone rename: restored to '{}' with sudo", name)
                                                            }
                                                        };
                                                        explorer.show_status(msg);
                                                        explorer.load_directory()?;
                                                    }
                                                    Err(e) => {
                                                        explorer.show_status(format!("Error: {}", e));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::ConfirmDelete { items } => {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    let items_to_delete = items.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    match explorer.perform_delete(&items_to_delete) {
                                        Ok(_) => {}
                                        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                                            explorer.ui_mode = UIMode::PasswordPrompt {
                                                prompt: "Permission denied. Enter sudo password:".to_string(),
                                                password: String::new(),
                                                pending_operation: Box::new(PendingOperation {
                                                    items: items_to_delete,
                                                    destination: None,
                                                    operation: OperationType::Delete,
                                                    undo_action: None,
                                                }),
                                            };
                                        }
                                        Err(e) => {
                                            explorer.show_status(format!("Error: {}", e));
                                        }
                                    }
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::RenameItem { original_path, new_name, cursor_pos, selection_start } => {
                            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                            match key.code {
                                KeyCode::Char(c) if !ctrl => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        // Delete selection if any
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            new_name.replace_range(start..end, "");
                                            *cursor_pos = start;
                                            *selection_start = None;
                                        }
                                        // Insert character at cursor
                                        new_name.insert(*cursor_pos, c);
                                        *cursor_pos += 1;
                                    }
                                }
                                KeyCode::Char('a') if ctrl => {
                                    // Select all
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        *selection_start = Some(0);
                                        *cursor_pos = new_name.len();
                                    }
                                }
                                KeyCode::Char('c') if ctrl => {
                                    // Copy selection to system clipboard
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            if start < end {
                                                let selected_text = new_name[start..end].to_string();
                                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                                    let _ = clipboard.set_text(selected_text);
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char('v') if ctrl => {
                                    // Paste from system clipboard
                                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                        if let Ok(clipboard_text) = clipboard.get_text() {
                                            if !clipboard_text.is_empty() {
                                                if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                                    // Delete selection if any
                                                    if let Some(sel_start) = selection_start {
                                                        let start = (*sel_start).min(*cursor_pos);
                                                        let end = (*sel_start).max(*cursor_pos);
                                                        new_name.replace_range(start..end, "");
                                                        *cursor_pos = start;
                                                        *selection_start = None;
                                                    }
                                                    // Insert clipboard content at cursor
                                                    new_name.insert_str(*cursor_pos, &clipboard_text);
                                                    *cursor_pos += clipboard_text.len();
                                                }
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char('x') if ctrl => {
                                    // Cut selection (copy + delete)
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            if start < end {
                                                let selected_text = new_name[start..end].to_string();
                                                // Copy to clipboard
                                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                                    let _ = clipboard.set_text(selected_text);
                                                }
                                                // Delete from text
                                                new_name.replace_range(start..end, "");
                                                *cursor_pos = start;
                                                *selection_start = None;
                                            }
                                        }
                                    }
                                }
                                KeyCode::Left => {
                                    if let UIMode::RenameItem { cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            // Start or extend selection
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                            if *cursor_pos > 0 {
                                                *cursor_pos -= 1;
                                            }
                                        } else {
                                            // Clear selection and move cursor
                                            *selection_start = None;
                                            if *cursor_pos > 0 {
                                                *cursor_pos -= 1;
                                            }
                                        }
                                    }
                                }
                                KeyCode::Right => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            // Start or extend selection
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                            if *cursor_pos < new_name.len() {
                                                *cursor_pos += 1;
                                            }
                                        } else {
                                            // Clear selection and move cursor
                                            *selection_start = None;
                                            if *cursor_pos < new_name.len() {
                                                *cursor_pos += 1;
                                            }
                                        }
                                    }
                                }
                                KeyCode::Home => {
                                    if let UIMode::RenameItem { cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                        } else {
                                            *selection_start = None;
                                        }
                                        *cursor_pos = 0;
                                    }
                                }
                                KeyCode::End => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if shift {
                                            if selection_start.is_none() {
                                                *selection_start = Some(*cursor_pos);
                                            }
                                        } else {
                                            *selection_start = None;
                                        }
                                        *cursor_pos = new_name.len();
                                    }
                                }
                                KeyCode::Backspace => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            // Delete selection
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            new_name.replace_range(start..end, "");
                                            *cursor_pos = start;
                                            *selection_start = None;
                                        } else if *cursor_pos > 0 {
                                            // Delete character before cursor
                                            new_name.remove(*cursor_pos - 1);
                                            *cursor_pos -= 1;
                                        }
                                    }
                                }
                                KeyCode::Delete => {
                                    if let UIMode::RenameItem { new_name, cursor_pos, selection_start, .. } = &mut explorer.ui_mode {
                                        if let Some(sel_start) = selection_start {
                                            // Delete selection
                                            let start = (*sel_start).min(*cursor_pos);
                                            let end = (*sel_start).max(*cursor_pos);
                                            new_name.replace_range(start..end, "");
                                            *cursor_pos = start;
                                            *selection_start = None;
                                        } else if *cursor_pos < new_name.len() {
                                            // Delete character at cursor
                                            new_name.remove(*cursor_pos);
                                        }
                                    }
                                }
                                KeyCode::Enter => {
                                    let path = original_path.clone();
                                    let name = new_name.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    if let Err(e) = explorer.rename_item(path, name) {
                                        explorer.show_status(format!("Error: {}", e));
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::CreateNew { creation_type, name } => {
                            match key.code {
                                KeyCode::Char(c) if creation_type.is_none() => {
                                    // First step: choosing file or directory
                                    match c {
                                        'f' | 'F' => {
                                            if let UIMode::CreateNew { creation_type, .. } = &mut explorer.ui_mode {
                                                *creation_type = Some(CreationType::File);
                                            }
                                        }
                                        'd' | 'D' => {
                                            if let UIMode::CreateNew { creation_type, .. } = &mut explorer.ui_mode {
                                                *creation_type = Some(CreationType::Directory);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                KeyCode::Char(c) if creation_type.is_some() => {
                                    // Second step: entering name
                                    if let UIMode::CreateNew { name, .. } = &mut explorer.ui_mode {
                                        name.push(c);
                                    }
                                }
                                KeyCode::Backspace if creation_type.is_some() => {
                                    if let UIMode::CreateNew { name, .. } = &mut explorer.ui_mode {
                                        name.pop();
                                    }
                                }
                                KeyCode::Enter if creation_type.is_some() => {
                                    let ctype = creation_type.clone().unwrap();
                                    let item_name = name.clone();
                                    explorer.ui_mode = UIMode::Normal;

                                    if let Err(e) = explorer.create_new_item(ctype, item_name) {
                                        explorer.show_status(format!("Error: {}", e));
                                    }
                                }
                                KeyCode::Esc => {
                                    explorer.ui_mode = UIMode::Normal;
                                }
                                _ => {}
                            }
                        }
                        UIMode::Normal | UIMode::StatusMessage { .. } => {
                            let shift = key.modifiers.contains(KeyModifiers::SHIFT);
                            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

                            match key.code {
                                KeyCode::Char('q') if ctrl => return Ok(()),
                                KeyCode::Up => explorer.move_up(shift),
                                KeyCode::Down => explorer.move_down(shift),
                                KeyCode::Enter => explorer.open_or_enter()?,
                                KeyCode::Right => explorer.enter_directory()?,
                                KeyCode::Left => explorer.go_to_parent()?,
                                KeyCode::Char(' ') if ctrl => {
                                    explorer.toggle_selection();
                                }
                                KeyCode::Char('c') if ctrl => {
                                    explorer.copy_selected();
                                }
                                KeyCode::Char('x') if ctrl => {
                                    explorer.cut_selected();
                                }
                                KeyCode::Char('v') if ctrl => {
                                    explorer.paste()?;
                                }
                                KeyCode::Char('n') if ctrl => {
                                    explorer.start_create_new();
                                }
                                KeyCode::Char('r') if ctrl => {
                                    explorer.start_rename();
                                }
                                KeyCode::Delete => {
                                    explorer.delete_selected();
                                }
                                KeyCode::Char('d') if ctrl => {
                                    explorer.delete_selected();
                                }
                                KeyCode::Char('z') if ctrl => {
                                    explorer.undo()?;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    if matches!(explorer.ui_mode, UIMode::Normal) {
                        match mouse.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                explorer.handle_mouse_down(
                                    mouse.row,
                                    mouse.column,
                                    mouse.modifiers,
                                    0,
                                );
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                explorer.handle_mouse_drag(
                                    mouse.row,
                                    mouse.column,
                                    0,
                                );
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                explorer.handle_mouse_up();
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let explorer = FileExplorer::new()?;
    let res = run_app(&mut terminal, explorer);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {:?}", err);
    }

    Ok(())
}
