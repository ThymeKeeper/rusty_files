//! File system operations module.
//!
//! This module contains functions for file operations like copy, move, delete,
//! rename, and sudo-elevated operations.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

use crate::types::UndoAction;

/// Get a unique path by appending (1), (2), etc. if the path already exists.
pub fn get_unique_path(dest_path: &PathBuf) -> PathBuf {
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

/// Copy a directory recursively.
pub fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Perform a file operation (copy or move) with tracking for undo.
pub fn perform_file_operation_tracked(
    items: &[PathBuf],
    destination: &PathBuf,
    is_move: bool,
) -> io::Result<(usize, UndoAction)> {
    let mut count = 0;
    let mut tracked_operations = Vec::new();
    let mut copied_files = Vec::new();

    for item in items {
        let file_name = item.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name")
        })?;
        let initial_dest_path = destination.join(file_name);
        // Get a unique path to avoid conflicts
        let dest_path = get_unique_path(&initial_dest_path);

        if is_move {
            fs::rename(item, &dest_path)?;
            tracked_operations.push((item.clone(), dest_path.clone()));
        } else {
            if item.is_dir() {
                copy_dir_recursive(item, &dest_path)?;
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

/// Get default content for a new file based on its extension.
pub fn get_default_file_content(filename: &str) -> String {
    // Get file extension
    let extension = if let Some(dot_pos) = filename.rfind('.') {
        &filename[dot_pos + 1..]
    } else {
        return "\n".to_string();
    };

    // Return appropriate default content based on extension
    match extension.to_lowercase().as_str() {
        "py" => "#!/usr/bin/env python3\n".to_string(),
        "sh" | "bash" => "#!/bin/bash\n".to_string(),
        "rb" => "#!/usr/bin/env ruby\n".to_string(),
        "pl" => "#!/usr/bin/env perl\n".to_string(),
        "js" | "ts" | "jsx" | "tsx" => "// \n".to_string(),
        "html" | "htm" => "<!DOCTYPE html>\n<html>\n<head>\n    <title></title>\n</head>\n<body>\n    \n</body>\n</html>\n".to_string(),
        "css" => "/* */\n".to_string(),
        "rs" => "fn main() {\n    \n}\n".to_string(),
        "c" | "cpp" | "cc" | "h" | "hpp" => "// \n".to_string(),
        "java" => "// \n".to_string(),
        "go" => "package main\n\nfunc main() {\n    \n}\n".to_string(),
        "md" | "markdown" => "# \n".to_string(),
        _ => "\n".to_string(),
    }
}

/// Validate a sudo password.
pub fn validate_sudo_password(password: &str) -> io::Result<()> {
    let mut child = Command::new("sudo")
        .arg("-kS")
        .arg("-v")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{}", password)?;
        stdin.flush()?;
        drop(stdin); // Explicitly close stdin
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

/// Perform delete operation with sudo (Unix only - uses gio trash with pkexec).
#[cfg(unix)]
pub fn perform_delete_sudo(
    items: &[PathBuf],
    password: &str,
) -> io::Result<Vec<PathBuf>> {
    validate_sudo_password(password)?;
    let mut deleted_files = Vec::new();

    for item in items {
        let item_str = item.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
        })?;

        // Try using gio trash with sudo
        let mut child = Command::new("sudo")
            .arg("-S")
            .arg("gio")
            .arg("trash")
            .arg(item_str)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", password)?;
            stdin.flush()?;
            drop(stdin);
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            // Fallback to rm if gio trash fails
            let mut child = Command::new("sudo")
                .arg("-S")
                .arg("rm")
                .arg("-rf")
                .arg(item_str)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;

            if let Some(mut stdin) = child.stdin.take() {
                writeln!(stdin, "{}", password)?;
                stdin.flush()?;
                drop(stdin);
            }

            let output = child.wait_with_output()?;
            if !output.status.success() {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
            }
        }

        deleted_files.push(item.clone());
    }
    Ok(deleted_files)
}

/// Perform delete operation with elevated privileges (Windows - not supported, use regular delete).
#[cfg(not(unix))]
pub fn perform_delete_sudo(
    items: &[PathBuf],
    _password: &str,
) -> io::Result<Vec<PathBuf>> {
    // On Windows, just try regular trash delete
    let mut deleted_files = Vec::new();
    for item in items {
        match trash::delete(item) {
            Ok(_) => {
                deleted_files.push(item.clone());
            }
            Err(e) => {
                return Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
            }
        }
    }
    Ok(deleted_files)
}

/// Perform rename operation with sudo.
pub fn perform_rename_sudo(
    original_path: &PathBuf,
    new_path: &PathBuf,
    password: &str,
) -> io::Result<()> {
    validate_sudo_password(password)?;

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
        stdin.flush()?;
        drop(stdin);
    }

    let output = child.wait_with_output()?;
    if !output.status.success() {
        let error_msg = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(io::ErrorKind::Other, error_msg.to_string()));
    }

    Ok(())
}

/// Perform undo operation with sudo.
pub fn perform_undo_sudo(action: &UndoAction, password: &str) -> io::Result<usize> {
    validate_sudo_password(password)?;

    let mut count = 0;
    match action {
        UndoAction::Copy { copied_files } => {
            for file in copied_files {
                if file.exists() {
                    let file_str = file.to_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                    })?;

                    let args = if file.is_dir() { vec!["-rf", file_str] } else { vec![file_str] };

                    let mut child = Command::new("sudo")
                        .arg("-S")
                        .arg("rm")
                        .args(&args)
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()?;

                    if let Some(mut stdin) = child.stdin.take() {
                        writeln!(stdin, "{}", password)?;
                        stdin.flush()?;
                        drop(stdin);
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
                        stdin.flush()?;
                        drop(stdin);
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
            #[cfg(unix)]
            {
                // On Unix, try to restore from trash using sudo gio command
                for original_path in deleted_files {
                    let original_str = original_path.to_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Invalid path")
                    })?;

                    // Use sudo with gio trash --restore (requires URI format)
                    // First, try to find the file in trash and restore it
                    let mut child = Command::new("sudo")
                        .arg("-S")
                        .arg("gio")
                        .arg("trash")
                        .arg("--restore")
                        .arg(original_str)
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()?;

                    if let Some(mut stdin) = child.stdin.take() {
                        writeln!(stdin, "{}", password)?;
                        stdin.flush()?;
                        drop(stdin);
                    }

                    let output = child.wait_with_output()?;
                    if !output.status.success() {
                        // If gio restore fails, provide helpful error message
                        let error_msg = String::from_utf8_lossy(&output.stderr);
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            format!("Cannot restore from trash: {}. Files may have been permanently deleted or trash may have been emptied.", error_msg)
                        ));
                    }

                    count += 1;
                }
            }
            #[cfg(not(unix))]
            {
                // On non-Unix systems (Windows), try using trash crate restore
                match trash::os_limited::list() {
                    Ok(trash_items) => {
                        let mut items_to_restore = Vec::new();

                        // Find matching items in trash
                        for original_path in deleted_files {
                            for item in trash_items.iter() {
                                if item.original_path() == *original_path {
                                    items_to_restore.push(item.clone());
                                    break;
                                }
                            }
                        }

                        if items_to_restore.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                "No matching items found in trash (may have been emptied)",
                            ));
                        }

                        let restore_count = items_to_restore.len();
                        match trash::os_limited::restore_all(items_to_restore) {
                            Ok(_) => {
                                count = restore_count;
                            }
                            Err(e) => {
                                return Err(io::Error::new(io::ErrorKind::Other, e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        return Err(io::Error::new(io::ErrorKind::Other, format!("Cannot access trash: {}", e)));
                    }
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
                    stdin.flush()?;
                    drop(stdin);
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

/// Perform a file operation with sudo.
pub fn perform_file_operation_sudo(
    items: &[PathBuf],
    destination: &PathBuf,
    is_move: bool,
    password: &str,
) -> io::Result<usize> {
    validate_sudo_password(password)?;

    let mut count = 0;
    for item in items {
        let file_name = item.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Invalid file name")
        })?;
        let initial_dest_path = destination.join(file_name);
        let dest_path = get_unique_path(&initial_dest_path);

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
            stdin.flush()?;
            drop(stdin);
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
