# Export formats

Export is part of the Rust binary and has no optional runtime dependencies.
Exports are generated from the current TUI session; resume a native-window
session in the TUI first if you need the richer export commands.

From the TUI:

```text
/export markdown workspace.md
/export text workspace.txt
/export json workspace.json
/export html workspace.html
/export pdf workspace.pdf
```

If the path is relative, it is resolved inside the configured workspace. The
Markdown, text, JSON, and HTML exporters include the full session history,
reasoning metadata where present, and tool-call metadata. The built-in PDF
writer produces a portable summary PDF without downloading a document or
rendering runtime. Relative export paths are workspace-contained; an absolute
path is accepted when the user explicitly supplies it.

The formats are deliberately loss-aware:

- Markdown and text are human-readable session reports.
- JSON preserves the structured session and tool metadata.
- HTML is a standalone report with escaped content.
- PDF is a compact summary rather than a pixel-perfect copy of the TUI.
