# Export formats

Export is part of the Rust binary and has no optional runtime dependencies.

From the TUI:

```text
/export markdown workspace.md
/export text workspace.txt
/export json workspace.json
/export html workspace.html
/export pdf workspace.pdf
```

If the path is relative, it is resolved inside the configured workspace. The Markdown, text, JSON, and HTML exporters include the full transcript and tool call metadata. The built in PDF writer produces a portable summary PDF without downloading a document or rendering runtime.
