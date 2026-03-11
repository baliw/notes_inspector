
## Project Descripiton
This is a TUI app to manage notes in various apps.  Starting off with support for Apple Notes and Obsidian.
It's written in Rust and uses Ratatui for the UI.  Let's call is "Notes Inspector".
When it launches, provide a selector for Apple Notes or Obsidian.  If the user selects Obsidian, have it bring up a folder selector that starts at ~.  When a folder is selected it tests for an Obsidian Vault.  If it's a vault it opens a tree view of the vault.  If it's not a vault then it shows the sub-folders to select.
When opening Apple Notes, show the tree structure on the left and allow branches to be selected and opened.
For either Apple Notes or Obsidian Notes, the display should have two frames.  The left frame is the tree view with selectable folders and notes.  The right frame is the note frame to preview what is in the note.  For images, convert the image to 16 lines or less of ANSI art.
Make a top frame above the tree and note frames to show stats about the current database of notes.
For Obsidian databases, provide an option button in the footer to open a window and start checking that all attachments are linked to in notes and suggest files that have no links to be pruned.
