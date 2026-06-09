# Skills in Rova

Skills are markdown prompt templates that inject domain-specific guidance into the conversation as system messages. They support `{param}` placeholder substitution for dynamic parameterization.

## What Are Skills?

A skill is a markdown file in `~/.config/rova/skills/` that describes how the LLM should behave for a specific task. When activated via `/skill use`, its content is prepended to every chat request as a system message.

Skills are the simplest way to customize Rova's behavior without writing code.

## Creating a Skill

### Directory

Skills live in `~/.config/rova/skills/` by default. Create the directory if it doesn't exist:

```sh
mkdir -p ~/.config/rova/skills
```

### File Format

A skill is a plain markdown (`.md`) file. The filename (without `.md`) becomes the skill name:

```
~/.config/rova/skills/
├── code-reviewer.md
├── poet.md
├── web-researcher.md
└── shell-expert.md
```

### Basic Skill

```markdown
<!-- ~/.config/rova/skills/poet.md -->
Always respond in rhyming couplets. Use vivid imagery and metaphor.
Prefer iambic pentameter where possible. End each response with a ~.
```

Usage:
```
/skill use poet
```

### Parameterized Skill

Skills support `{key}` placeholders that are substituted when the skill is activated:

```markdown
<!-- ~/.config/rova/skills/code-reviewer.md -->
You are a {language} code reviewer. Apply the following standards:
- Style guide: {style_guide}
- Focus areas: {focus_areas}
- Severity threshold: {severity}

For each issue found, report: file, line, severity, description, and fix.
```

Usage:
```
/skill use code-reviewer language=Rust style_guide="rustfmt + clippy" focus_areas="correctness,unsafe blocks" severity=high
```

The system message injected into the conversation becomes:

```
Active skill: code-reviewer
You are a Rust code reviewer. Apply the following standards:
- Style guide: rustfmt + clippy
- Focus areas: correctness,unsafe blocks
- Severity threshold: high

For each issue found, report: file, line, severity, description, and fix.
```

### Placeholder Rules

- Placeholder names: `{key}` — alphanumeric, underscores, hyphens
- Substitution is literal string replacement (no evaluation, no recursion)
- Unmatched placeholders are left as-is (the LLM sees the raw `{key}`)
- Values can contain spaces when quoted: `key="value with spaces"`

## Commands

| Command | Description |
|---------|-------------|
| `/skills` | List all available skill files |
| `/skill use <name> [key=value ...]` | Activate a skill with optional parameters |
| `/skill show <name>` | Print the raw content of a skill file |
| `/skill drop <name>` | Deactivate one skill |
| `/skill clear` | Deactivate all skills |

### Multiple Skills

You can activate multiple skills simultaneously. They are injected as separate system messages in order of activation:

```
/skill use poet
/skill use web-researcher query="how CPUs work"
```

Both skills' system messages are included in each chat request. If they conflict, the LLM resolves the conflict — typically the later or more specific instruction takes precedence.

## Skill Lifecycle

1. **Load:** `/skill use <name>` reads the `.md` file and adds it to `ChatState.active_skills`
2. **Inject:** Before every chat request, `_skill_messages()` reads each active skill and builds system messages
3. **Parameterize:** `{key}` placeholders are substituted with values from `ChatState.skill_params`
4. **Deactivate:** `/skill drop` removes the skill; `/skill clear` removes all

Skills are **not persisted** across sessions. Re-activate them on each launch, or use the `--skills-dir` CLI flag to set a custom location.

## Example Skill Files

### Code Reviewer

```markdown
<!-- code-reviewer.md -->
You are a {language} code reviewer.

For each issue found, report:
- **File:** path
- **Line:** number
- **Severity:** critical | major | minor | style
- **Issue:** description
- **Fix:** concrete suggestion

Focus on: {focus_areas}
```

### Web Researcher

```markdown
<!-- web-researcher.md -->
When answering questions, follow this process:
1. Break the question into search queries
2. Use web_search for each query
3. Use web_fetch for the top 3 results
4. Synthesize findings with citations (URL + snippet)
5. Note any gaps or uncertainties

Query: {query}
```

### Shell Expert

```markdown
<!-- shell-expert.md -->
You are a Linux shell expert. When writing commands:
- Prefer POSIX-compatible syntax unless {shell} is specified
- Explain each flag and pipeline stage
- Warn about destructive operations (rm, dd, mkfs)
- Show expected output format
- Suggest safer alternatives when available

Target shell: {shell}
Target OS: {os}
```

## Security

Skills are read from the local filesystem and injected as system messages. They are **not** executable code — they're prompt templates. The only risk is prompt injection if you activate a skill from an untrusted source, so review skill content before activation.

The skill loader blocks path traversal:

```python
def read_skill(skills_dir: Path, name: str, params: dict | None = None) -> str:
    if "/" in name or "\\" in name or name.startswith("."):
        return ""  # blocks ../../etc/passwd style attacks
```
