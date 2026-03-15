use crate::tree::{NodeKind, TreeNode};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Live progress reported by `find_vaults_with_progress`.
#[derive(Debug)]
pub struct ScanProgress {
    pub folders_searched: usize,
    pub current_path: String,
}

pub type SharedScanProgress = Arc<Mutex<ScanProgress>>;

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
        } else {
            // Any non-note file is treated as an attachment
            let mut att = TreeNode::new_note(name, path);
            att.kind = NodeKind::Attachment;
            node.children.push(att);
        }
    }
}

/// Image extensions for inline rendering.
const IMAGE_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "tiff", "heic",
];

/// Read the content of a note file, resolving `![[image]]` embeds to inline image markers.
pub fn read_note(path: &Path) -> String {
    let content = fs::read_to_string(path).unwrap_or_else(|e| format!("Error reading file: {e}"));
    let vault_root = find_vault_root(path);
    resolve_image_embeds(&content, path, vault_root.as_deref())
}

/// Walk up from a note path to find the vault root (directory containing `.obsidian/`).
fn find_vault_root(note_path: &Path) -> Option<PathBuf> {
    let mut dir = note_path.parent()?;
    loop {
        if dir.join(".obsidian").is_dir() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

/// Replace `![[file]]` and `![alt](path)` image embeds with `__INLINE_IMAGE__:` markers.
fn resolve_image_embeds(content: &str, note_path: &Path, vault_root: Option<&Path>) -> String {
    let note_dir = note_path.parent().unwrap_or(note_path);
    let mut result = String::with_capacity(content.len());
    let bytes = content.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Check for ![[...]] embed
        if i + 3 < bytes.len() && bytes[i] == b'!' && bytes[i + 1] == b'[' && bytes[i + 2] == b'[' {
            if let Some(end) = content[i + 3..].find("]]") {
                let reference = &content[i + 3..i + 3 + end];
                let file_part = reference.split('|').next().unwrap_or(reference);
                let file_part = file_part.split('#').next().unwrap_or(file_part).trim();

                if let Some(resolved) = resolve_to_image(file_part, note_dir, vault_root) {
                    result.push_str("__INLINE_IMAGE__:");
                    result.push_str(&resolved.to_string_lossy());
                    result.push('\n');
                    i += 3 + end + 2;
                    continue;
                }
            }
        }

        // Check for ![alt](path) embed
        if bytes[i] == b'!' && i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            if let Some(bracket_end) = content[i + 2..].find("](") {
                let paren_start = i + 2 + bracket_end + 2;
                if let Some(paren_end) = content[paren_start..].find(')') {
                    let path_str = content[paren_start..paren_start + paren_end].trim();
                    if !path_str.starts_with("http") {
                        if let Some(resolved) = resolve_to_image(path_str, note_dir, vault_root) {
                            result.push_str("__INLINE_IMAGE__:");
                            result.push_str(&resolved.to_string_lossy());
                            result.push('\n');
                            i = paren_start + paren_end + 1;
                            continue;
                        }
                    }
                }
            }
        }

        result.push(bytes[i] as char);
        i += 1;
    }
    result
}

/// Try to resolve a file reference to an image path. Returns None if not an image or not found.
fn resolve_to_image(reference: &str, note_dir: &Path, vault_root: Option<&Path>) -> Option<PathBuf> {
    let ext = Path::new(reference)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if !IMAGE_EXTS.contains(&ext.as_str()) {
        return None;
    }

    // Try relative to note directory
    let from_note = note_dir.join(reference);
    if from_note.exists() {
        return Some(from_note);
    }

    // Try relative to vault root
    if let Some(root) = vault_root {
        let from_root = root.join(reference);
        if from_root.exists() {
            return Some(from_root);
        }

        // Search vault-wide by filename
        let fname = Path::new(reference)
            .file_name()?
            .to_string_lossy()
            .to_string();
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file()
                && entry.file_name().to_string_lossy() == fname
            {
                return Some(entry.path().to_path_buf());
            }
        }
    }

    None
}

/// Result of attachment analysis.
#[derive(Debug)]
#[allow(dead_code)]
pub struct AttachmentAnalysis {
    pub total_attachments: usize,
    pub linked_attachments: usize,
    pub unlinked: Vec<PathBuf>,
}

/// Find all attachments in the vault and check which ones are referenced in notes.
#[allow(dead_code)]
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
    find_vaults_recursive(root, max_depth, &mut vaults, None);
    vaults.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    vaults
}

/// Like `find_vaults`, but reports live progress via a shared handle.
pub fn find_vaults_with_progress(
    root: &Path,
    max_depth: u32,
    progress: &SharedScanProgress,
) -> Vec<PathBuf> {
    let mut vaults = Vec::new();
    find_vaults_recursive(root, max_depth, &mut vaults, Some(progress));
    vaults.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    vaults
}

fn find_vaults_recursive(
    dir: &Path,
    depth: u32,
    vaults: &mut Vec<PathBuf>,
    progress: Option<&SharedScanProgress>,
) {
    if depth == 0 {
        return;
    }
    if let Some(p) = progress {
        let mut prog = p.lock().unwrap();
        prog.folders_searched += 1;
        prog.current_path = dir.to_string_lossy().to_string();
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
        find_vaults_recursive(&path, depth - 1, vaults, progress);
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

/// A config file from the `.obsidian/` directory.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    pub name: String,
    pub content: String,
}

/// List and read all JSON/config files from the `.obsidian/` directory.
pub fn read_vault_config(vault_path: &Path) -> Vec<ConfigFile> {
    let config_dir = vault_path.join(".obsidian");
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(&config_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Ok(content) = fs::read_to_string(&path) {
                    files.push(ConfigFile { name, content });
                }
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

/// A single issue found during vault integrity checking.
#[derive(Debug, Clone)]
pub enum IntegrityIssue {
    /// A `[[link]]` or `[text](path)` that doesn't resolve to any file.
    BrokenLink {
        source_note: PathBuf,
        link_target: String,
    },
    /// An attachment file not referenced by any note.
    UnlinkedAttachment { path: PathBuf },
}

/// Result of a full vault integrity check.
#[derive(Debug)]
pub struct IntegrityResult {
    pub issues: Vec<IntegrityIssue>,
    pub notes_scanned: usize,
    pub attachments_scanned: usize,
    pub broken_links: usize,
    pub unlinked_attachments: usize,
}

/// Run a full integrity check on a vault: broken links + unlinked attachments.
pub fn check_integrity(vault_path: &Path) -> IntegrityResult {
    let mut all_notes: Vec<PathBuf> = Vec::new();
    let mut all_attachments: Vec<PathBuf> = Vec::new();
    // Index: filename -> exists, stem -> exists
    let mut file_names: HashSet<String> = HashSet::new();
    let mut file_stems: HashSet<String> = HashSet::new();
    // Relative paths from vault root (with and without extension)
    let mut relative_paths: HashSet<String> = HashSet::new();
    // Track which files are referenced by notes (for unlinked attachment detection)
    let mut referenced_files: HashSet<String> = HashSet::new();
    let mut issues: Vec<IntegrityIssue> = Vec::new();

    // Walk vault, skipping hidden directories entirely
    for entry in walkdir::WalkDir::new(vault_path)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let fname = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        file_names.insert(fname.clone());
        file_stems.insert(stem);

        if let Ok(rel) = path.strip_prefix(vault_path) {
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            relative_paths.insert(rel_str.clone());
            // Also store without extension
            if let Some(pos) = rel_str.rfind('.') {
                relative_paths.insert(rel_str[..pos].to_string());
            }
        }

        if is_note_file(path) {
            all_notes.push(path.to_path_buf());
        } else {
            all_attachments.push(path.to_path_buf());
        }
    }

    // Check every link in every note
    for note_path in &all_notes {
        let content = match fs::read_to_string(note_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let note_dir = note_path.parent().unwrap_or(vault_path);

        // Wiki-link targets: resolved vault-wide
        let wiki_targets = extract_wiki_link_targets(&content);
        for target in &wiki_targets {
            if !link_resolves(target, vault_path, note_dir, &file_names, &file_stems, &relative_paths) {
                issues.push(IntegrityIssue::BrokenLink {
                    source_note: note_path.clone(),
                    link_target: format!("[[{target}]]"),
                });
            }
        }

        // Standard markdown link targets: resolved relative to note, then vault root
        let md_targets = extract_md_link_targets(&content);
        for target in &md_targets {
            if !link_resolves(target, vault_path, note_dir, &file_names, &file_stems, &relative_paths) {
                issues.push(IntegrityIssue::BrokenLink {
                    source_note: note_path.clone(),
                    link_target: target.clone(),
                });
            }
        }

        // Collect attachment references for unlinked check
        extract_references(&content, &mut referenced_files);
    }

    // Find unlinked attachments
    for att in &all_attachments {
        let fname = att
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !referenced_files.contains(&fname) {
            issues.push(IntegrityIssue::UnlinkedAttachment {
                path: att.clone(),
            });
        }
    }

    let broken_links = issues
        .iter()
        .filter(|i| matches!(i, IntegrityIssue::BrokenLink { .. }))
        .count();
    let unlinked_attachments = issues
        .iter()
        .filter(|i| matches!(i, IntegrityIssue::UnlinkedAttachment { .. }))
        .count();

    IntegrityResult {
        issues,
        notes_scanned: all_notes.len(),
        attachments_scanned: all_attachments.len(),
        broken_links,
        unlinked_attachments,
    }
}

/// Try to resolve a link target against the vault's file index.
fn link_resolves(
    target: &str,
    vault_path: &Path,
    note_dir: &Path,
    file_names: &HashSet<String>,
    file_stems: &HashSet<String>,
    relative_paths: &HashSet<String>,
) -> bool {
    // Direct filename or stem match (vault-wide)
    if file_names.contains(target) || file_stems.contains(target) {
        return true;
    }
    // Relative path match (e.g. "subfolder/Note Name")
    let normalized = target.replace('\\', "/");
    if relative_paths.contains(&normalized) {
        return true;
    }
    // Try with .md extension
    let with_md = format!("{normalized}.md");
    if file_names.contains(&with_md) || relative_paths.contains(&with_md) {
        return true;
    }
    // Resolve relative to note directory
    let from_note = note_dir.join(target);
    if from_note.exists() {
        return true;
    }
    // Resolve relative to vault root
    let from_vault = vault_path.join(target);
    if from_vault.exists() {
        return true;
    }
    false
}

/// Extract wiki-link targets: `[[target]]` and `![[target]]`.
fn extract_wiki_link_targets(content: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len().saturating_sub(1) {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(end) = content[i + 2..].find("]]") {
                let reference = &content[i + 2..i + 2 + end];
                let file_part = reference.split('|').next().unwrap_or(reference);
                let file_part = file_part.split('#').next().unwrap_or(file_part);
                let file_part = file_part.trim();
                if !file_part.is_empty() {
                    targets.push(file_part.to_string());
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }
    targets
}

/// Extract standard markdown link targets: `[text](path)`, excluding URLs and anchors.
fn extract_md_link_targets(content: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut search_from = 0;
    while let Some(start) = content[search_from..].find("](") {
        let abs_start = search_from + start + 2;
        if let Some(end) = content[abs_start..].find(')') {
            let path = content[abs_start..abs_start + end].trim();
            if !path.is_empty() && !path.starts_with("http") && !path.starts_with('#') {
                targets.push(path.to_string());
            }
            search_from = abs_start + end + 1;
        } else {
            break;
        }
    }
    targets
}

/// Delete a file from disk.
pub fn delete_file(path: &Path) -> Result<(), String> {
    fs::remove_file(path).map_err(|e| format!("Failed to delete {}: {e}", path.display()))
}
