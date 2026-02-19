# Rule Sync: Claude Code ↔ Cursor

Rules are maintained in two locations that must stay in sync:

- **Claude Code**: `.claude/rules/*.md`
- **Cursor IDE**: `.cursor/rules/*.mdc`

When creating or updating any rule file, you MUST apply the identical change to both locations. The only difference between the two copies is the file extension (`.md` vs `.mdc`) and that Cursor files may include YAML frontmatter (e.g., `alwaysApply: true`).

## Procedure

1. Make the change in the file you're working with.
2. Immediately mirror the change to the corresponding file in the other directory.
3. If creating a new rule, create it in both directories.
4. If deleting a rule, delete it from both directories.
