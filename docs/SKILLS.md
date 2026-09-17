# Skills

Skills are local Markdown prompt templates. r105 reads them from the configured
skills directory and injects active skills as system messages before the
conversation history. Skills are prompt data: they do not run by themselves
and cannot bypass mode, permission, or approval gates.

## Location and format

The default directory is `~/.config/r105/skills`; `skills_dir` can point to a
workspace or shared directory. A skill is one Markdown file:

```sh
mkdir -p ~/.config/r105/skills
cat > ~/.config/r105/skills/reviewer.md <<'EOF'
Review the requested change for correctness, security, and missing tests.
Return findings with file, line, impact, and a concrete fix.
EOF
```

Skill names are a single local filename. Path separators, absolute paths, dot names, and parent traversal are rejected.

## Commands

| Command | Behavior |
| --- | --- |
| /skills | List Markdown files |
| /skill use <name> [key=value ...] | Activate a skill and substitute parameters |
| /skill show <name> | Display the raw skill |
| /skill drop <name> | Deactivate one skill |
| /skill clear | Deactivate all skills |

Example:

```text
/skills
/skill use reviewer
/skill show reviewer
/skill drop reviewer
```

## Parameters

A skill can contain literal placeholders:

```markdown
You are reviewing a {language} project.
Focus on {areas}.
```

Activate it with quoted values when needed:

```text
/skill use reviewer language=Rust areas="security and tests"
```

Values are literal replacements. They are stored in the session state and
injected on the next request. A skill name is only a single local filename;
path separators, absolute paths, dot names, and parent traversal are rejected.
Skills are prompt text; r105 does not execute them. A skill may ask the model
to use tools, but the normal mode and approval policy still applies.

Global skills are shared by launches. Project-specific skills can live in a
workspace-configured directory; keep secrets and credentials out of Markdown
prompt files because they become part of model context.
