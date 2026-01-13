//! File-based clipboard for cross-instance copy/paste.
//!
//! This module provides a simple file-based IPC mechanism that allows
//! copying and pasting files between different rusty_files instances.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::types::{Clipboard, ClipboardOp};

/// Get the path to the clipboard file.
fn get_clipboard_file_path() -> io::Result<PathBuf> {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "Could not find home directory"))?;

    Ok(PathBuf::from(home).join(".rusty_files_clipboard"))
}

/// Write clipboard data to the shared clipboard file.
pub fn write_clipboard(clipboard: &Clipboard) -> io::Result<()> {
    let clipboard_path = get_clipboard_file_path()?;

    let mut file = fs::File::create(clipboard_path)?;

    // Write operation type (copy or cut)
    let op_str = match clipboard.operation {
        ClipboardOp::Copy => "copy",
        ClipboardOp::Cut => "cut",
    };
    writeln!(file, "{}", op_str)?;

    // Write file paths (one per line)
    for path in &clipboard.items {
        if let Some(path_str) = path.to_str() {
            writeln!(file, "{}", path_str)?;
        }
    }

    Ok(())
}

/// Read clipboard data from the shared clipboard file.
pub fn read_clipboard() -> io::Result<Option<Clipboard>> {
    let clipboard_path = get_clipboard_file_path()?;

    // If file doesn't exist, return None
    if !clipboard_path.exists() {
        return Ok(None);
    }

    let file = fs::File::open(clipboard_path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Read operation type
    let operation = match lines.next() {
        Some(Ok(line)) => {
            match line.trim() {
                "copy" => ClipboardOp::Copy,
                "cut" => ClipboardOp::Cut,
                _ => return Ok(None), // Invalid format
            }
        }
        _ => return Ok(None), // Empty file or error
    };

    // Read file paths
    let mut items = Vec::new();
    for line in lines {
        if let Ok(path_str) = line {
            let path = PathBuf::from(path_str.trim());
            // Only include paths that still exist
            if path.exists() {
                items.push(path);
            }
        }
    }

    // If no valid items, return None
    if items.is_empty() {
        return Ok(None);
    }

    Ok(Some(Clipboard { items, operation }))
}
