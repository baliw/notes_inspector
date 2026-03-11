use crate::tree::{NodeKind, TreeNode};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Check if a directory is an Obsidian vault (has .obsidian folder).
pub fn is_obsidian_vault(path: &Path) -> bool {
    path.join(".obsidian").is_dir()
}

/// Common attachment extensions.
const ATTACHMENT_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "mp3", "mp4", "wav", "ogg", "pdf", "zip",
    "tar", "gz",
];

/// Markdown/note extensions.
const NOTE_EXTS: &[&str] = &["md", "markdown", "txt"];

fn is_note_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| NOTE_EXTS.contains(&ext.to_lowercase().as_str()))
}

fn is_attachment_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ATTACHMENT_EXTS.contains(&ext.to_lowercase().as_str()))
}

/// Build a tree from an Obsidian vault directory.
pub fn build_vault_tree(vault_path: &Path) -> TreeNode {
    let name = vault_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut root = TreeNode::new_folder(name, vault_path.to_path_buf());
    root.expanded = true;
    populate_folder(&mut root, vault_path);
    root.sort_children();
    root
}

fn populate_folder(node: &mut TreeNode, dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files/folders (like .obsidian, .git, .trash)
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            let mut folder = TreeNode::new_folder(name, path.clone());
            populate_folder(&mut folder, &path);
            // Only add folders that contain notes or attachments
            if !folder.children.is_empty() {
                node.children.push(folder);
            }
        } else if is_note_file(&path) {
            let display_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            node.children.push(TreeNode::new_note(display_name, path));
        } else if is_attachment_file(&path) {
            let mut att = TreeNode::new_note(name, path);
            att.kind = NodeKind::Attachment;
            node.children.push(att);
        }
    }
}

/// Read the content of a note file.
pub fn read_note(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("Error reading file: {e}"))
}

/// Result of attachment analysis.
#[derive(Debug)]
pub struct AttachmentAnalysis {
    pub total_attachments: usize,
    pub linked_attachments: usize,
    pub unlinked: Vec<PathBuf>,
}

/// Find all attachments in the vault and check which ones are referenced in notes.
pub fn analyze_attachments(vault_path: &Path) -> AttachmentAnalysis {
    let mut all_attachments: Vec<PathBuf> = Vec::new();
    let mut referenced_files: HashSet<String> = HashSet::new();

    // Walk the vault
    for entry in walkdir::WalkDir::new(vault_path)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if name.starts_with('.') {
            continue;
        }

        if path.is_file() {
            if is_attachment_file(path) {
                all_attachments.push(path.to_path_buf());
            } else if is_note_file(path) {
                // Parse note for attachment references
                if let Ok(content) = fs::read_to_string(path) {
                    extract_references(&content, &mut referenced_files);
                }
            }
        }
    }

    let unlinked: Vec<PathBuf> = all_attachments
        .iter()
        .filter(|att| {
            let fname = att
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            !referenced_files.contains(&fname)
        })
        .cloned()
        .collect();

    let total = all_attachments.len();
    AttachmentAnalysis {
        total_attachments: total,
        linked_attachments: total - unlinked.len(),
        unlinked,
    }
}

/// Extract file references from markdown content.
/// Handles both `![[file.png]]` (Obsidian) and `![alt](file.png)` (standard) syntax.
fn extract_references(content: &str, refs: &mut HashSet<String>) {
    // Obsidian-style: ![[filename]] or [[filename]]
    let mut i = 0;
    let bytes = content.as_bytes();
    while i < bytes.len().saturating_sub(1) {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end) = content[i + 2..].find("]]") {
                let reference = &content[i + 2..i + 2 + end];
                // Handle aliases: [[file|alias]]
                let file_part = reference.split('|').next().unwrap_or(reference);
                // Handle headings: [[file#heading]]
                let file_part = file_part.split('#').next().unwrap_or(file_part);
                let file_part = file_part.trim();
                if !file_part.is_empty() {
                    // Add with and without extension
                    refs.insert(file_part.to_string());
                    if let Some(pos) = file_part.rfind('.') {
                        refs.insert(file_part[..pos].to_string());
                    }
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }

    // Standard markdown: ![alt](path)
    let mut search_from = 0;
    while let Some(start) = content[search_from..].find("](") {
        let abs_start = search_from + start + 2;
        if let Some(end) = content[abs_start..].find(')') {
            let path = content[abs_start..abs_start + end].trim();
            if !path.is_empty() && !path.starts_with("http") {
                // Extract just the filename
                if let Some(fname) = Path::new(path).file_name() {
                    refs.insert(fname.to_string_lossy().to_string());
                }
            }
            search_from = abs_start + end + 1;
        } else {
            break;
        }
    }
}

/// Directories to skip when scanning for vaults (large/irrelevant).
const SKIP_DIRS: &[&str] = &[
    "Library",
    "node_modules",
    ".Trash",
    ".git",
    ".cargo",
    ".rustup",
    ".npm",
    ".cache",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
    ".local",
    "go",
    ".docker",
    "target",
    "build",
    "dist",
    "vendor",
];

/// Scan for Obsidian vaults under `root` up to `max_depth` levels deep.
pub fn find_vaults(root: &Path, max_depth: u32) -> Vec<PathBuf> {
    let mut vaults = Vec::new();
    find_vaults_recursive(root, max_depth, &mut vaults);
    vaults.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    vaults
}

fn find_vaults_recursive(dir: &Path, depth: u32, vaults: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if is_obsidian_vault(&path) {
            vaults.push(path);
            // Don't recurse into vaults (nested vaults are unusual)
            continue;
        }
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        find_vaults_recursive(&path, depth - 1, vaults);
    }
}

/// Initialize a directory as an Obsidian vault by creating `.obsidian/`.
pub fn init_vault(path: &Path) -> Result<(), String> {
    let obsidian_dir = path.join(".obsidian");
    fs::create_dir_all(&obsidian_dir)
        .map_err(|e| format!("Failed to create .obsidian directory: {e}"))
}

/// List subdirectories of a path (for folder selection).
pub fn list_subdirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.starts_with('.') {
                    dirs.push(p);
                }
            }
        }
    }
    dirs.sort();
    dirs
}
