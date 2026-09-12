# Skills

Skills are local Markdown prompt templates. r105 reads them from the configured skills directory and injects active skills as system messages before the conversation history.

## Location and format

The default directory is ~/.config/r105/skills. A skill is one Markdown file:

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

Values are literal replacements. They are stored in the session state and injected on the next request. Skills are prompt text; r105 does not execute them.
