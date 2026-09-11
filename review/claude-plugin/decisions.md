# Decisions on claude-plugin

## Plugin placement in dusk repository
- decided: Harness lives at dusk/claude-plugin/, registered as marketplace "dusk" via .claude/settings.json
- alternatives: Separate repository; ~/.claude/skills directory; unregistered plugin with --plugin-dir only
- reversal: Move directory, change 2 config keys — costs 5 minutes
- triage: ask-human — Structural decision about repository layout and plugin discoverability that affects users' installation path and future maintenance; reversal is cheap so the question is low-cost.

## Move CLAUDE.md into claude-plugin/ directory
- decided: Root CLAUDE.md becomes a one-liner referencing @claude-plugin/CLAUDE.md; Claude Code imports from the new location natively
- alternatives: Inject via SessionStart hook as additionalContext; keep it at .claude/CLAUDE.md (original location)
- reversal: git mv claude-plugin/CLAUDE.md back and rm root CLAUDE.md — one-line change
- triage: decide-alone — Internal tooling decision with zero downstream impact; all three options are functionally identical; reversal cost is a single git command.

## Consolidate plugin skills location
- decided: All seven skills (5 workflow + 2 domain: authoring-a-program, adding-a-driver-method) move into claude-plugin/skills/; .claude/ keeps only settings.json
- alternatives: Move only 5 workflow skills to plugin (leave 2 domain skills in .claude/skills); leave all skills in .claude/skills, ship only hooks and agents in claude-plugin/
- reversal: git mv skills back; update config; reversal cost is 5–10 minutes total
- triage: ask-human — Repository layout is structural; CLAUDE.md names "layout of crates or modules across the tree" as ask-human even when reversal is cheap.

## Hook system implementation approach (Python, modular, shared harness)
- decided: Hooks as individual Python 3 scripts (one per event) sharing harness.py; block via exit code 2; ask/updatedInput use JSON format
- alternatives: Bash with jq; one script dispatching on hook_event_name; prompt- or agent-type hooks
- reversal: Rewrite all hook scripts from Python to alternative language/pattern; hooks.json configuration stays valid; ~1–2 hours per alternative
- triage: ask-human — This is a structural design choice (language + organization pattern) with meaningful alternatives; CLAUDE.md says "You are muscle, not pilot" and requires asking before picking an approach; layout decisions require approval.

## Harness rule enforcement strategy (deny vs. ask)
- decided: Hard-deny for commit format/git commands/timeout; ask for cargo test/sign-off/merge
- alternatives: Hard-deny all rules; warn-only through additionalContext
- reversal: Flip rules in pre_bash.py—a few-line diff in one file
- triage: ask-human — This is a structural workflow decision about harness strictness that belongs to the user, not an implementation detail Claude can choose.

## Skill-enforcement gate via pre_edit hook
- decided: Deny base/ edits until authoring-a-program loaded; for Driver changes, nested haiku call (cached) gates adding-a-driver-method
- alternatives: Prompt-type hook instead; apply haiku to base/ as well; no enforcement (rely on user discipline)
- reversal: Delete enforcement branch in pre_edit.py
- triage: ask-human — This is a policy decision about what to gate and how aggressively—harness architecture—and the user decides the developer workflow constraints.

## Decision-tracking persistence infrastructure
- decided: Evaluating three approaches to how decisions are recorded (file vs. PR vs. command) and automated (hooks vs. model vs. bin tool)
- alternatives: (1) Model writes decisions/<branch>.md directly; (2) Decisions live only in PR body, no committed file; (3) bin/decide command manages recording; (4) keep current hook-based system
- reversal: Switching to any approach costs ~5–10 minutes (rewrite hook writers, remove hook invocations, or remove file-write code), but different choices have different implications for auditability, discoverability, and reproducibility across sessions
- triage: ask-human — This is harness infrastructure that affects how decisions persist in the repository and whether they remain discoverable after a PR merges; the user set up the current system and should decide its shape, not Claude.

## Branch-name keying with worktree- prefix stripped
- decided: Drive state keyed by branch name minus leading worktree-, matching git workflow (local branch worktree-<name> pushed as <name>)
- alternatives: Rename the local branch to avoid stripping logic, or key by session ID instead
- reversal: Edit one function in harness.py
- triage: decide-alone — Internal harness implementation detail with low reversal cost, not exposed as a public boundary, not affecting users or structural design.

## Stop hook cap on repeated failed blocks
- decided: MAX_BLOCKS = 3 in stop.py; halts if three identical blocks fail without progress
- alternatives: No cap (infinite loop risk), or cap of 1 (may force premature exit)
- reversal: Change MAX_BLOCKS constant in stop.py
- triage: decide-alone — Internal harness mechanism, easily reversible, choosing between two equivalent loop-guard strategies.

## Lazy skill injection for drive-issue
- decided: drive-issue skill injected on-demand (UserPromptSubmit when issue named) instead of at SessionStart
- alternatives: inject full skill every SessionStart (wastes context), or never inject (skill invisible)
- reversal: edit session_start.py, ~5 lines
- triage: decide-alone — Internal optimization about context loading, not user-visible or structural; lazy wins on budget and the skill works identically when invoked.

## Accept gh CLI for PR body checks
- decided: Corrected the drive-issue hook docs to acknowledge gh 2.46.0 is installed and accept both gh and GitHub MCP as valid tools for checking PR bodies
- alternatives: Keep the MCP-only rule and revert the correction
- reversal: Revert the corrected paragraph; one-line change to documentation/configuration
- triage: decide-alone — Non-structural tooling decision with trivial reversal cost; affects only internal harness, not APIs, schemas, or user-facing behavior.

## Skip eval suite, rely on self-test + strict validation
- decided: No evals/ suite ships; hooks have hooks/selftest.py (28 cases) + claude plugin validate --strict
- alternatives: Ship evals/ suite now
- reversal: Adding evals/ directory later costs nothing; pure addition
- triage: decide-alone — Omitting optional test infrastructure is within standard authority (YAGNI, ponytail lazy); no API/schema/structural impact; deferred addition has no reversal cost.

## Agent capability allowlists in pre_bash.py
- decided: Restrict read-only agents (self-review, race-screen, race-inspector, dilemma-triage, decision-ranker) from mutating git/files; permit atomic-commit to edit/commit but not push
- alternatives: No restrictions (agents have full access); permissionMode plan (different governance model)
- reversal: Edit frontmatter metadata and one set in pre_bash.py—a few-line diff
- triage: ask-human — This sets policy for what multiple agents in the sign-off system can do—structural governance that affects the entire harness, consistent with how similar agent-restriction decisions (skill-enforcement-gate, harness-rule-enforcement) have required approval.

## race-screen self-invokes race-inspector
- decided: race-screen gains Agent tool and dispatches race-inspector as a subagent when conditions warrant, rather than a parent agent deciding dispatch
- alternatives: main/parent agent controls when race-inspector runs; race-inspector runs unconditionally on every invocation
- reversal: modify race-screen's prompt and available tools; reroute dispatch responsibility
- triage: ask-human — This is an architectural choice about agent orchestration responsibility (bottom-up vs top-down control), which CLAUDE.md reserves for the pilot, not the muscle.
