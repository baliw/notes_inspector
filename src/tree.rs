use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum NodeKind {
    Folder,
    Note,
    Attachment,
    /// Visual separator line (not selectable).
    Divider,
}

#[derive(Debug, Clone)]
pub struct TreeNode {
    pub name: String,
    pub path: PathBuf,
    pub kind: NodeKind,
    pub children: Vec<TreeNode>,
    pub expanded: bool,
    /// Cocoa timestamp (seconds since 2001-01-01).  Only set for notes.
    pub modified_date: Option<f64>,
    /// Whether this note is pinned/starred.
    pub is_pinned: bool,
}

impl TreeNode {
    pub fn new_folder(name: String, path: PathBuf) -> Self {
        Self {
            name,
            path,
            kind: NodeKind::Folder,
            children: Vec::new(),
            expanded: false,
            modified_date: None,
            is_pinned: false,
        }
    }

    pub fn new_note(name: String, path: PathBuf) -> Self {
        Self {
            name,
            path,
            kind: NodeKind::Note,
            children: Vec::new(),
            expanded: false,
            modified_date: None,
            is_pinned: false,
        }
    }

    pub fn new_divider() -> Self {
        Self {
            name: String::new(),
            path: PathBuf::new(),
            kind: NodeKind::Divider,
            children: Vec::new(),
            expanded: false,
            modified_date: None,
            is_pinned: false,
        }
    }

    /// Sort children: folders first (alphabetical), then notes.
    /// Notes are split into pinned (by date desc) + divider + unpinned (by date desc).
    pub fn sort_children(&mut self) {
        // Recurse first
        for child in &mut self.children {
            child.sort_children();
        }

        // Separate by kind
        let mut folders: Vec<TreeNode> = Vec::new();
        let mut notes: Vec<TreeNode> = Vec::new();
        let mut attachments: Vec<TreeNode> = Vec::new();

        for child in self.children.drain(..) {
            match child.kind {
                NodeKind::Folder => folders.push(child),
                NodeKind::Note => notes.push(child),
                NodeKind::Attachment => attachments.push(child),
                NodeKind::Divider => {} // drop old dividers on re-sort
            }
        }

        // Sort folders alphabetically
        folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Split notes into pinned / unpinned, each sorted by date desc
        let mut pinned: Vec<TreeNode> = Vec::new();
        let mut unpinned: Vec<TreeNode> = Vec::new();
        for n in notes {
            if n.is_pinned {
                pinned.push(n);
            } else {
                unpinned.push(n);
            }
        }

        let by_date_desc = |a: &TreeNode, b: &TreeNode| {
            let da = a.modified_date.unwrap_or(0.0);
            let db = b.modified_date.unwrap_or(0.0);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        };
        pinned.sort_by(by_date_desc);
        unpinned.sort_by(by_date_desc);

        // Reassemble: folders, pinned notes, divider (if both exist), unpinned notes, attachments
        self.children.extend(folders);
        if !pinned.is_empty() {
            self.children.extend(pinned);
            if !unpinned.is_empty() {
                self.children.push(TreeNode::new_divider());
            }
        }
        self.children.extend(unpinned);
        self.children.extend(attachments);
    }

    pub fn count_notes(&self) -> usize {
        let mut count = 0;
        if self.kind == NodeKind::Note {
            count += 1;
        }
        for child in &self.children {
            count += child.count_notes();
        }
        count
    }

    pub fn count_folders(&self) -> usize {
        let mut count = 0;
        if self.kind == NodeKind::Folder {
            count += 1;
        }
        for child in &self.children {
            count += child.count_folders();
        }
        count
    }

    pub fn count_attachments(&self) -> usize {
        let mut count = 0;
        if self.kind == NodeKind::Attachment {
            count += 1;
        }
        for child in &self.children {
            count += child.count_attachments();
        }
        count
    }
}

/// A flattened view of the tree for display purposes.
#[derive(Debug, Clone)]
pub struct FlatItem {
    pub depth: usize,
    pub name: String,
    pub kind: NodeKind,
    pub expanded: bool,
    pub has_children: bool,
    /// Index path into the tree to locate this node.
    pub index_path: Vec<usize>,
    /// Cocoa modification timestamp (for notes).
    pub modified_date: Option<f64>,
    /// Whether this note is pinned.
    pub is_pinned: bool,
}

/// Flatten a tree into a list for rendering.
pub fn flatten_tree(roots: &[TreeNode]) -> Vec<FlatItem> {
    let mut items = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        flatten_node(root, 0, &mut vec![i], &mut items);
    }
    items
}

fn flatten_node(
    node: &TreeNode,
    depth: usize,
    path: &mut Vec<usize>,
    items: &mut Vec<FlatItem>,
) {
    items.push(FlatItem {
        depth,
        name: node.name.clone(),
        kind: node.kind.clone(),
        expanded: node.expanded,
        has_children: !node.children.is_empty(),
        index_path: path.clone(),
        modified_date: node.modified_date,
        is_pinned: node.is_pinned,
    });

    if node.expanded {
        for (i, child) in node.children.iter().enumerate() {
            path.push(i);
            flatten_node(child, depth + 1, path, items);
            path.pop();
        }
    }
}

/// Get a mutable reference to a tree node by its index path.
pub fn get_node_mut<'a>(roots: &'a mut [TreeNode], path: &[usize]) -> Option<&'a mut TreeNode> {
    if path.is_empty() {
        return None;
    }
    let mut node = roots.get_mut(path[0])?;
    for &idx in &path[1..] {
        node = node.children.get_mut(idx)?;
    }
    Some(node)
}

/// Get a reference to a tree node by its index path.
pub fn get_node<'a>(roots: &'a [TreeNode], path: &[usize]) -> Option<&'a TreeNode> {
    if path.is_empty() {
        return None;
    }
    let mut node = roots.get(path[0])?;
    for &idx in &path[1..] {
        node = node.children.get(idx)?;
    }
    Some(node)
}
