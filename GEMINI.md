# Workspace Overview: stint & Claude Workflow

This workspace is a hybrid environment containing a personal time-tracking system and a sophisticated AI agent orchestration framework.

## 1. stint Time Tracker
A suite of tools for tracking billable work hours and generating invoices ($16.00/hr flat rate).

### Key Components
- **`stint.sh`**: Main Bash entry point. Handles timer lifecycle (`start`, `stop`, `status`), manual logging (`add`), and report generation (`log`, `invoice`).
- **`legacy-consolidate.py`**: Python engine for grouping entries by ticket ID (e.g., `T123`, `PR #45`) or category, rounding hours to the nearest 0.25h, and generating HTML invoices.
- **`stint-YYYY-MM.csv`**: Monthly log files storing raw entry data.
- **`temp/`**: Contains invoice templates (`invoice-dynamic.html`), staging files (`staging-YYYY-MM.txt`), and generated outputs.

### Common Commands
```bash
./stint.sh start "Description" [category]   # Start a timer
./stint.sh stop                             # Stop and log current timer
./stint.sh log 2026 05                      # View log for May 2026
./stint.sh consolidate 2026 05              # Prepare for invoicing (creates staging file)
./stint.sh invoice 2026 05 --html           # Generate HTML invoice
```

---

## 2. Claude Workflow (`.claude/`)
A structured framework designed to optimize AI agent performance and maintain codebase health.

### Core Principles
- **Context Hygiene**: Minimal `CLAUDE.md` for navigation; detailed `README.md` for architectural "invisible knowledge."
- **Planning Before Execution**: Mandatory planning phase to surface ambiguities before code is written.
- **Review Cycles**: Multi-agent review (Technical Writer, Quality Reviewer) for all plans and implementations.
- **Skill-Based Orchestration**: Specialized tools (`deepthink`, `planner`, `refactor`) driven by Python scripts.

### Agent Architecture: The "Book" Pattern
Python scripts in `.claude/skills/scripts/` follow a strict "Book" pattern:
- Files read top-to-bottom without forward references.
- Sections are ordered: Prompts → Config → Templates → Builders → Logic → Steps → Entry Point.
- Workflows are step-delimited and table-driven.

### Testing Agent Skills
The workflow includes its own test suite for validating agent logic and prompts.
- **Location**: `.claude/skills/scripts/tests/`
- **Runner**: `pytest`

---

## 3. Directory Structure Triggers
- **Root**: Core stint logic and data logs.
- **`.claude/skills/`**: Source for agent capabilities. Each skill has a `SKILL.md` entry point.
- **`.claude/conventions/`**: Global rules for documentation, naming, and code quality.
- **`.claude/agents/`**: System prompts for specialized sub-agents (Developer, Architect, etc.).

## 4. Development Conventions
- **Comments**: Use the "Timeless Present" rule (e.g., "Saves the file" instead of "Saved the file").
- **Documentation**: Adhere to the token-budgeted two-file pattern (see `.claude/conventions/documentation.md`).
- **Intent Markers**: Use markers like `:PERF:`, `:UNSAFE:`, or `:SCHEMA:` to signal critical code characteristics.
