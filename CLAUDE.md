
## Project Descripiton
This is a TUI app to manage notes in various apps.  Starting off with support for Apple Notes and Obsidian.
It's written in Rust and uses Ratatui for the UI.  Let's call is "Notes Inspector".
When it launches, provide a selector for Apple Notes or Obsidian.  If the user selects Obsidian, have it bring up a folder selector that starts at ~.  When a folder is selected it tests for an Obsidian Vault.  If it's a vault it opens a tree view of the vault.  If it's not a vault then it shows the sub-folders to select.
When opening Apple Notes, show the tree structure on the left and allow branches to be selected and opened.
For either Apple Notes or Obsidian Notes, the display should have two frames.  The left frame is the tree view with selectable folders and notes.  The right frame is the note frame to preview what is in the note.  For images, convert the image to 16 lines or less of ANSI art.
Make a top frame above the tree and note frames to show stats about the current database of notes.
For Obsidian databases, provide an option button in the footer to open a window and start checking that all attachments are linked to in notes and suggest files that have no links to be pruned.


## Best Practices
- Spend extra time keeping the code readable, documented, structured, and as simple as straight forward as possible.
  - Strive to reduce code bloat
- Always detect the number of CPU cores available and use at least half of them for thread pools if there's something in the app that can benefit from parallel execution.
  - Make thread count configurable on the command line
- Always keep the UI responsive.
  - For any process that is going to take some time, kick it off in a thread and provide status updates while keeping the UI usable.
- For any process where the user might wait for more than a second, provide status and a throbber.
- for any preview screens (screens or windows that show data but do not offer moodification) when they scroll use the up and down arrow key to move the view up and down by 5 lines at a time.

## TUI Best Practices
- Layout & Structure
  - Use consistent grid/column alignment. Terminals are character-based grids — align elements to columns religiously. Misaligned text destroys perceived quality instantly.
  - Establish clear visual hierarchy. Use whitespace, borders, and typography weight (bold, dim) to separate primary content from secondary content from chrome (headers, footers, status bars).
  - Reserve predictable screen zones. A common pattern: top bar for title/context, main body for content, bottom bar for keybindings/status. Users should never be disoriented about where they are.
  - Avoid overly dense UIs. Terminals feel claustrophobic quickly. Use padding inside panels and margin between components generously — at least 1 character of breathing room.
  - Design for 80-column width as baseline. Many users run narrow terminals or split panes. Gracefully handle narrower widths; never hard-wrap or truncate critical content unexpectedly.
- Navigation & Interaction
  - Follow established keybinding conventions. Don't reinvent navigation. h/j/k/l or arrow keys for movement, q to quit, / to search, ? for help, Enter to confirm, Esc to cancel/go back. Violating muscle memory kills UX.
  - Make every action discoverable. Display a persistent keybinding hint bar (à la nano, htop, lazygit). Users shouldn't need to memorize or read a man page to use basic features.
  - Implement a help overlay (?). A full-screen or popup help menu listing all keybindings is expected in any non-trivial TUI.
  - Never trap the user. There should always be a way out — q, Ctrl+C, or Esc should never dead-end. Implement graceful exit at every screen level.
  - Use modal UI purposefully. If you have modes (like Vim's insert/normal), make the current mode extremely obvious via a persistent indicator. Invisible modes are a cardinal sin.
- Visual Design
  - Use color semantically, not decoratively. Assign meaning to colors: red = error/danger, yellow = warning, green = success, cyan = info/selection, white = default. Don't use colors just for aesthetics if they carry no meaning.
  - Always provide a no-color/monochrome fallback. Respect the NO_COLOR environment variable (nocolor.org standard). Pipes, CI systems, and accessibility needs require color-free output.
  - Use bold and dim text instead of color when possible. Bold for emphasis, dim for metadata/secondary info. This works universally regardless of terminal color theme.
  - Avoid hardcoding foreground/background colors. Use terminal-default backgrounds (Default color) so your UI respects the user's terminal theme (dark/light). Hardcoding black backgrounds on a light-theme terminal is jarring.
  - Use Unicode box-drawing characters for borders. ─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼ are universally supported. They look far more polished than ASCII +-|.
  - Distinguish focused vs unfocused panels clearly. Use a different border color or style (e.g., bold border vs dim border) to show which pane has focus.
  - Use icons/symbols sparingly and with text fallbacks. Nerd Font glyphs (icons) are great but not universally available. Either detect font support or provide ASCII fallbacks.
- Performance and Responsiveness
  - Never block the main/render thread. Run I/O, network calls, and heavy computation in background goroutines/threads/async tasks. A frozen TUI is dead UX.
  - Implement incremental/partial rendering. Only redraw what changed. Full-screen redraws on every keystroke cause visible flicker, especially over SSH.
  - Show loading indicators for any operation > ~100ms. A spinner or progress bar prevents users from thinking the app has crashed. braille spinner, dots, or a simple percentage bar all work well.
  - Debounce rapid input events. For search-as-you-type or resize events, debounce to avoid unnecessary re-renders.
  - Handle terminal resize gracefully. Listen for SIGWINCH (Unix) or equivalent. Reflow content and re-render immediately on resize. Never leave a half-rendered layout after resizing.
- Text & Content
  - Truncate, don't wrap, in table/list views. Long strings in lists should be truncated with … to preserve alignment. Wrapping destroys scannable column layouts.
  - Right-align numbers and left-align text in tables. Standard data table convention. Numbers (sizes, counts, percentages) should be right-aligned for easy comparison.
  - Use relative time formatting for recency. "2 hours ago" is more scannable than "2026-03-13 11:42:07 UTC" in most list contexts. Offer a toggle if exact time matters.
  - Highlight search matches. If you support search/filter, highlight matching characters in results. This is expected and dramatically improves usability.
  - Scroll indicators are mandatory for long content. Show a scrollbar glyph or (n/N) position indicator whenever content overflows. Users need to know there's more below.
- State & Feedback
  - Give immediate visual feedback for every action. Key presses, selections, confirmations — all should have instant visual acknowledgment. No silent actions.
  - Use a persistent status/notification area. Transient messages (success, error, info) should appear in a dedicated area (typically the bottom bar) and auto-dismiss or be clearable. Don't flash and immediately destroy important messages.
  - Confirm destructive actions. Deletions, overwrites, and irreversible operations should require a confirmation prompt (y/N), defaulting to the safe option.
  - Preserve state across navigation. When users go back to a previous screen, restore their scroll position and selection. Losing context on back-navigation is frustrating.
- Compatibility & Environment
  - Detect terminal capabilities before using them. Check $TERM, $COLORTERM, and tput capabilities. Don't assume 256-color or true color support — gracefully degrade to 8-color or monochrome.
  - Test over SSH. Many users run TUIs remotely. Latency, encoding differences, and lack of certain capabilities are all real. Test with ssh over a throttled connection.
  - Respect $TERM, $COLUMNS, $LINES, and $PAGER. Use the environment the user has configured, don't override it.
  - Handle Ctrl+C/SIGINT cleanly. Restore terminal state (disable raw mode, show cursor, clear alternate screen) before exit. A TUI that leaves the terminal in a broken state is unforgivable.
  - Use the alternate screen buffer. Switch to the alternate screen (smcup/rmcup) on launch and restore the original screen on exit. Users should return to exactly what they had in their terminal before launching your app.
  - Always restore the cursor. If you hide the cursor during rendering (civis), always show it again (cnorm) on exit — including on panics/crashes.
- Misc
  - Separate rendering logic from business logic. Your UI components should be pure render functions driven by state. This makes testing possible and avoids spaghetti.
  - Make the UI testable. Snapshot test your rendered output. At minimum, write integration tests that simulate keypresses and assert resulting state.
  - Log to a file, not stdout/stderr. In raw terminal mode, printing to stdout destroys the UI. Write debug logs to a file (e.g., ~/.local/share/myapp/debug.log) or use a debug overlay panel.





