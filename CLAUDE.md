# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`ggcommons` is the **Greengrass Commons** ecosystem: a set of libraries, a scaffolding CLI, and component templates for building AWS IoT Greengrass v2 components. The libraries bundle the cross-cutting concerns every component needs — configuration, messaging, metrics, heartbeat, logging — behind service interfaces so component authors write only business logic.

This directory is a **workspace of independent projects**, not a single buildable repo (the workspace root is not a git repo; some subprojects are). Each subproject has its own build system, CI, and (for the Python library) its own detailed `CLAUDE.md`.

| Directory | What it is | Stack |
|-----------|-----------|-------|
| `ggcommons-java-lib/` | The Java library (canonical, most complete). PyPI/Maven artifact `com.aws.proserve:ggcommons`. | Java 25 (LTS), Maven |
| `ggcommons-python-lib/` | The Python port (PyPI `greengrass-commons`), being brought to feature parity with Java. **Has its own `CLAUDE.md` — read it before working here.** | Python 3.8+, setuptools |
| `ggcommons-cli/` | Scaffolding CLI that generates new components from templates. | Python |
| `java-component-skeleton/`, `python-component-skeleton/` | Worked example components ("best practices demos") using the libraries. | Java / Python |
| `java-componen-template/`, `python-component-template/` | Minimal starting templates the CLI copies (note the typo in the Java template dir name). | Java / Python |

The Java and Python libraries are deliberate mirrors of each other (same subsystems, same CLI contract, same config schema). When changing public behavior in one, check whether the other needs the matching change to preserve parity.

## Core concepts (shared across all subprojects)

Every component built with these libraries runs in one of two **runtime modes**, selected at startup via `-m/--mode`. Most of the architecture exists to abstract this difference away:
- **GREENGRASS** (default): uses Greengrass IPC for messaging; reads config from the Greengrass deployment.
- **STANDALONE**: for Kubernetes/Docker/bare containers. Uses a dual-MQTT provider that connects simultaneously to a local broker and to AWS IoT Core. Requires a separate messaging-config JSON file (e.g. `standalone-messaging-sample.json`).

**Standard CLI contract** (same in both languages — keep them aligned):
- `-c/--config <SOURCE> [args...]` — one of `FILE`, `ENV`, `GG_CONFIG` (default), `SHADOW`, `CONFIG_COMPONENT`.
- `-m/--mode <MODE> [path]` — `GREENGRASS` (default) or `STANDALONE <messaging_config.json>`. STANDALONE without a path is a hard error.
- `-t/--thing <name>` — IoT Thing name; must take the full string value (historical bug truncated it to one char).

**Shared subsystems** (each library has parallel packages for these): `config/` (5 config-source managers + template-variable substitution, hot reload, multi-instance, JSON-schema validation), `messaging/` (IPC vs dual-MQTT providers behind one interface; connections/subscriptions block until confirmed), `metrics/` (pluggable targets: CloudWatch EMF, messaging, local log), `heartbeat/` (periodic system metrics via injected services), a service-interface seam (idiomatic trait/`interface` injection in **Rust and TS only** — `di/`+`interfaces/` packages do **not** exist in Java or Python; see the parity register `.validation/parity-remediation-plan.md`), and fluent **builders** for object construction.

## Commands

### Python library (`ggcommons-python-lib/`)
```bash
pip install -r requirements.txt -r requirements-test.txt && pip install -e .
python -m pytest                                  # all tests (config in pytest.ini; very verbose, log_cli=DEBUG)
python -m pytest tests/test_builders.py::TestMessageBuilder::test_build -v   # single test
python run_pytest.py --coverage                   # convenience wrapper (coverage, file/function selection)
python -m pytest -m "not slow and not integration and not aws"   # skip slow/AWS-dependent
```
`ruff` and `black` (target py39–py311) are configured but **not enforced in CI** (the lint steps in `.gitlab-ci.yml` are commented out) — match that formatting manually. CI only builds the wheel and publishes to the GitLab PyPI registry. See `ggcommons-python-lib/CLAUDE.md` for the full architecture.

### Java library (`ggcommons-java-lib/`)
```bash
mvn clean package            # build + test → shaded self-contained JAR
mvn clean package -DskipTests
mvn test -Dtest=ClassName#methodName   # single test
mvn clean install            # install to local ~/.m2
```
The Shade plugin produces a self-contained JAR suitable for Greengrass deployment. CI (`.gitlab-ci.yml`) builds with `maven:3.9-amazoncorretto-11` and **skips tests** (`-Dmaven.test.skip=true`), deploying artifacts only from the default branch.

### Components (skeletons & templates)
Components are packaged/deployed with the **GDK (Greengrass Development Kit)**, configured per-component in `gdk-config.json` (`build_system`: `maven` for Java, `zip` for Python) and `recipe.yaml` (the Greengrass component recipe — declares default config and IPC `accessControl`). Typical flow: `gdk component build` then `gdk component publish`. Run a built component locally:
```bash
# Python skeleton
python3 main.py -m STANDALONE standalone-messaging.json -c FILE config.json -t my-thing
# Java skeleton
java -jar target/<artifact>.jar -m STANDALONE ./standalone-messaging.json -c FILE ./test-configs/config_2.json -t my-thing
```

### CLI (`ggcommons-cli/`)
```bash
pip install -r requirements.txt
./scripts/ggcommons-cli.sh create_component --name MyNewComponent   # or .cmd / .ps1 on Windows
```
The CLI auto-discovers commands by scanning `commands/*.py` for classes implementing `execute_command` + `get_json_configuration` (see `ggcommons_cli.py`). To add a command, drop a new module in `commands/` following that contract.

## Local development with MQTT

STANDALONE mode and local testing use a local MQTT broker standing in for Greengrass IPC:
```bash
docker run -d --name emqx -p 1883:1883 -p 8083:8083 -p 8883:8883 -p 18083:18083 emqx/emqx:latest
```
Use an MQTT client (e.g. MQTTX) to subscribe to `heartbeat/+/+` to see component heartbeats and to drive request/response topics. If a component hard-depends on other components, run those locally too.

## Conventions

- **Maintain Java↔Python parity.** The two libraries mirror each other intentionally; the same config schema, CLI flags, and subsystem boundaries apply to both.
- **Backward compatibility is preserved.** Builders are the construction path in all four libs. The legacy `ggcommons.init(...)` / direct-constructor API coexists **only in Java** (Rust/TS are builder-only greenfield; Python is builder/constructor-only with no `init()` facade). Don't break the old surface when adding the new one.
- **Service-interface seam (Rust/TS).** Rust (`MessagingService`/`MetricService` traits + `Arc<dyn …>` injection) and TS (`IMessagingService`/`MetricService` interfaces + constructor injection) provide a substitutable seam; **Java and Python do not** have `IConfigurationService`/`IMessagingService`/`IMetricService` or a `ServiceRegistry` (Java's DI was removed during remediation; Python never shipped it despite older docs). In Java/Python, test against the concrete services / process-global statics (`MessagingClient`, `MetricEmitter`), whose state leaks across tests unless reset.
- **Construct via builders**, not raw constructors (`GGCommonsBuilder`, `MessageBuilder`, `MetricBuilder`, etc.). `MetricBuilder` specifically replaces the deprecated direct `Metric` constructor.
- Python tests are pytest-style (`Test*` classes, `test_*` functions) — the suite was migrated off `unittest`; don't add new `unittest.TestCase` subclasses.
- Per-subsystem docs live under each library's `doc/` (architecture, messaging, configuration, metric-emission, heartbeat, logging, etc.). Update the relevant doc when changing a subsystem's public behavior.

This file defines strict behavioral rules for all AI coding agents (Junie, Claude Code, Cursor, Grok, etc.) working in this project. Agents **must** follow these rules at all times.

## Karpathy's Core Recommendations (Adapted)
Derived from Andrej Karpathy's observations on LLM/agent coding pitfalls and best practices for agentic engineering.

1. **Think Before Coding**
    - Do not assume. State your assumptions explicitly.
    - Do not hide confusion. Surface uncertainties, tradeoffs, and potential issues immediately.
    - If anything is unclear, ask one clarifying question and wait for confirmation before proceeding.
    - Prefer simpler solutions and push back on over-engineering.

2. **Simplicity First**
    - Implement the minimum code that solves the exact problem requested.
    - Avoid speculative features, unnecessary abstractions, premature optimization, or "flexibility" that wasn't asked for.
    - If a task can be done in 50 lines, do not write 200.

3. **Surgical Changes**
    - Touch only the code/files necessary for the requested task.
    - Never refactor, reformat, or "improve" adjacent/unrelated code unless explicitly asked.
    - Every changed line in a diff must directly trace back to the user's request.

4. **Goal-Driven Execution**
    - Explicitly restate the task as verifiable success criteria.
    - Work in clear, iterative loops until all criteria are met.
    - Prioritize macro actions and high-leverage changes.

## Mandatory Verification & Quality Workflow
In addition to Karpathy's principles, follow this rigorous process for **every** code change:

1. **API Verification**
    - Before using any crate, function, or external API, verify its current behavior, parameters, and best practices (e.g., via official docs, `cargo doc --open`, or quick tests).
    - Confirm compatibility with the project's Rust version and dependencies.

2. **Compile After Every Update**
    - After any code modification, immediately run `cargo check` or `cargo build`.
    - Fix all compilation errors before proceeding.
    - Do not leave broken code.

3. **Full Testing**
    - For every new or modified feature/function:
        - Write comprehensive **unit tests** (using Rust's built-in `#[test]` framework).
        - Write relevant **integration tests** (in `tests/` directory if applicable).
        - Run the full test suite with `cargo test` (or `cargo nextest` if available).
    - Tests must cover happy paths, edge cases, and error conditions.
    - Fix failing tests and re-run until everything passes.
    - Provide the test output in your response.

## Rust Documentation Requirements
**All agents must follow these documentation standards strictly for every public API and module.**

### Module-Level Documentation
Every module (`mod.rs`, `lib.rs`, or any `mod` declaration) must include comprehensive module-level documentation at the top using the following template:

```rust
//! # Module Name
//!
//! **One-liner purpose**: High-level description of what this module does.
//!
//! ## Overview
//! Detailed explanation of the module's responsibilities, design decisions,
//! and how it fits into the larger system.
//!
//! ## Semantics & Architecture
//! - Core invariants maintained by this module
//! - Thread-safety guarantees
//! - Async vs sync usage expectations
//! - Error handling strategy (e.g., `anyhow`, `thiserror`, custom errors)
//!
//! ## Usage Example
//! ```rust
//! use crate::this_module;
//!
//! # tokio_test::main;
//! async fn example() -> anyhow::Result<()> {
//!     // ...
//!     Ok(())
//! }
//! ```
//!
//! ## Algorithmic / Design Choices
//! - Why this approach was chosen over alternatives
//! - Trade-offs considered (performance vs simplicity, etc.)
//!
//! ## Safety & Panics
//! Any conditions that may cause panics (should be rare in safe Rust).
//!
//! ## Related Modules
//! Links or references to closely related modules.
```

### Function-Level Documentation
Every public function (and significant private ones) must be documented using this exact template:
```rust
/// Brief one-line purpose of the function.
///
/// # Purpose
/// More detailed explanation of *what* the function accomplishes and *why* it exists.
///
/// # Semantics & Syntax
/// - **Signature**: `pub async fn function_name(param1: Type1, param2: Type2) -> Result<ReturnType, ErrorType>`
/// - Detailed description of parameters, return value, and behavior.
/// - Ownership semantics (takes ownership, borrows, etc.).
///
/// # Pre-conditions
/// - List all assumptions that must be true before calling this function.
/// - Examples: valid paths, initialized state, non-empty collections, permissions, etc.
///
/// # Post-conditions
/// - Guaranteed outcomes if pre-conditions are met and no error is returned.
/// - State changes, invariants preserved, etc.
///
/// # Algorithmic Choices
/// - High-level description of the algorithm/approach used.
/// - Why this algorithm was selected (e.g., "WAL mode chosen for better concurrency").
/// - Time/space complexity (if relevant).
/// - Any notable optimizations or simplifications.
///
/// # Errors
/// | Error Variant | Condition | Recovery Suggestion |
/// |---------------|---------|---------------------|
/// | `Error::Io`   | File system permission denied or disk full | Check permissions and disk space |
/// | `Error::Sqlx` | Database constraint violation | Validate input before calling |
/// | ...           | ...     | ... |
///
/// # Examples
/// ```rust
/// # use anyhow::Result;
/// # async fn demo() -> Result<()> {
/// let result = function_name("valid_input").await?;
/// # Ok(())
/// # }
/// ```
///
/// # Panics
/// (Rare) Conditions under which this function may panic.
```

### Additional Documentation Rules

Always use rustdoc compatible syntax (/// for items, //! for modules).
Keep documentation accurate and up-to-date with every change.
Generate and verify docs with cargo doc --open when appropriate.
Document structs, traits, enums, and important constants using similar detail where relevant.

# General Rules

Always prefer idiomatic, safe Rust (ownership, borrowing, error handling with anyhow/thiserror, etc.).
Maintain consistency with existing code style and architecture (e.g., sqlx + Tokio patterns).
Document key decisions and tradeoffs.

Follow these rules strictly to produce reliable, maintainable, production-grade, well-documented code.

