# Export Extra

r105 core installs with minimal dependencies for terminal use.

## Optional export dependencies

Document export features are opt-in:

```bash
pip install "r105[export]"
```

This installs:
- `python-pptx`
- `python-docx`
- `fpdf2`
- `pillow`

### Usage

Once installed, the `/export` slash commands and `export_conversation` will work for all formats.

If the extra is not installed, the CLI will show a helpful error suggesting the install command.
