# oss-context: Java Library Documentation MCP Server

## Overview

**oss-context** is a Rust MCP server that provides on-demand documentation for open-source Java libraries. It exposes 3 tools over stdio or SSE transport. When asked about a library, it resolves documentation through a chain: javadoc JARs (local → remote) → source JARs (local → remote), analyses the content, stores structured data in SQLite with FTS5, and deletes the JAR.

## Architecture

```
┌─────────────┐     MCP (stdio/SSE)     ┌──────────────────┐
│  LLM Client │ ◄──────────────────────► │   oss-context    │
└─────────────┘                          │                  │
                                         │  ┌────────────┐  │
                                         │  │ MCP Layer   │  │
                                         │  ├────────────┤  │
                                         │  │ Doc Engine  │  │
                                         │  ├────────────┤  │
                                         │  │ Source Repo │  │
                                         │  │ Resolution  │  │
                                         │  ├────────────┤  │
                                         │  │ JAR Parser  │  │
                                         │  │ & Analyzer  │  │
                                         │  ├────────────┤  │
                                         │  │ SQLite FTS5 │  │
                                         │  └────────────┘  │
                                         └──────────────────┘
                                           ▼           ▼
                                     ~/.m2/repo    Maven Central
                                     ~/.gradle/    Remote repos
                                     settings.xml  pom.xml
                                     build.gradle
```

### Crate Structure

Single binary, split into modules:

- `transport` — stdio/SSE setup via `rmcp`
- `tools` — the 3 MCP tool handlers
- `resolver` — finds JARs (local → remote), manages downloads + cleanup
- `parser` — extracts docs from javadoc JARs and source JARs
- `store` — SQLite schema, FTS5 indexing, queries
- `discovery` — auto-detects local repos, parses `settings.xml`/`pom.xml`/`build.gradle`
- `config` — layered config (auto-discover → config file → CLI)

## MCP Tools

### `resolve_library`

- **Params:** `query` (string — e.g. "guava 33.0" or "com.google.guava:guava:33.0.0-jre")
- **Behaviour:**
  1. Parse input — try `groupId:artifactId:version` first, fall back to fuzzy name matching against known indexed libraries and Maven Central search API
  2. Check if already indexed in SQLite → return immediately with status `ready`
  3. If not indexed, trigger the resolution chain: javadoc JAR (local → remote) → source JAR (local → remote)
  4. Download to temp dir, extract, analyse, populate SQLite, delete JAR
  5. Return library ID + status (`ready`, `indexing`, `not_found`)
- **Note:** Indexing blocks for v1. Background indexing can be added later.

### `query_docs`

- **Params:** `library_id` (string, from resolve step), `query` (string, free-text question)
- **Returns:** Top matching doc entries via FTS5 ranked by relevance — class/method signatures, doc comments, annotations, parameter info
- **Limit:** Returns top 20 results by default, configurable

### `browse_library`

- **Params:** `library_id` (string), `path` (optional string)
  - `""` or omitted → list packages
  - `"com.google.common.collect"` → list classes in package
  - `"com.google.common.collect.ImmutableList"` → full class documentation with all members
- **Returns:** Hierarchical navigation — package list → class list → full class documentation

## Resolution Chain

Stops at first success:

```
1. Javadoc JAR (local repos)
   ├─ ~/.m2/repository/{g}/{a}/{v}/{a}-{v}-javadoc.jar
   └─ ~/.gradle/caches/modules-2/files-2.1/{g}/{a}/{v}/*-javadoc.jar
2. Javadoc JAR (remote repos)
   ├─ Auto-discovered from settings.xml / pom.xml / build.gradle
   └─ Maven Central (default fallback)
3. Source JAR (local repos)
   ├─ ~/.m2/repository/{g}/{a}/{v}/{a}-{v}-sources.jar
   └─ ~/.gradle/caches/.../*-sources.jar
4. Source JAR (remote repos)
   └─ Same remote resolution as step 2
5. Not found → return error
```

### Javadoc JAR Parsing

- Extract to temp dir
- Parse HTML files using `scraper` crate — javadoc HTML has predictable structure
- Extract signatures, doc text, param descriptions, annotations
- Populate SQLite, delete temp dir

### Source JAR Parsing

- Extract `.java` files to temp dir
- Parse ASTs with `tree-sitter-java` — extract:
  - Public/protected classes, interfaces, enums, records
  - Method signatures, parameter names and types
  - Field declarations
  - Javadoc comments attached to declarations
  - Annotations
  - Inheritance hierarchy (`extends`/`implements`)
- Populate SQLite, delete temp dir

## SQLite Schema

One database file per library version at `~/.cache/oss-context/docs/{groupId}/{artifactId}/{version}.db`

```sql
CREATE TABLE library (
    group_id    TEXT NOT NULL,
    artifact_id TEXT NOT NULL,
    version     TEXT NOT NULL,
    source_type TEXT NOT NULL,  -- 'javadoc' or 'source'
    indexed_at  TEXT NOT NULL   -- ISO 8601
);

CREATE TABLE package (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    doc_comment TEXT
);

CREATE TABLE type (
    id          INTEGER PRIMARY KEY,
    package_id  INTEGER NOT NULL REFERENCES package(id),
    name        TEXT NOT NULL,          -- simple name
    fqn         TEXT NOT NULL UNIQUE,   -- fully qualified name
    kind        TEXT NOT NULL,          -- class/interface/enum/annotation/record
    doc_comment TEXT,
    annotations TEXT,                   -- JSON array
    superclass  TEXT,                   -- FQN or null
    interfaces  TEXT                    -- JSON array of FQNs
);

CREATE TABLE method (
    id          INTEGER PRIMARY KEY,
    type_id     INTEGER NOT NULL REFERENCES type(id),
    name        TEXT NOT NULL,
    signature   TEXT NOT NULL,          -- full signature with param types
    return_type TEXT,
    params      TEXT NOT NULL,          -- JSON array [{name, type, doc}]
    doc_comment TEXT,
    annotations TEXT,                   -- JSON array
    is_static   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE field (
    id          INTEGER PRIMARY KEY,
    type_id     INTEGER NOT NULL REFERENCES type(id),
    name        TEXT NOT NULL,
    field_type  TEXT NOT NULL,
    doc_comment TEXT,
    annotations TEXT,
    is_static   INTEGER NOT NULL DEFAULT 0
);

CREATE VIRTUAL TABLE docs_fts USING fts5(
    fqn,
    kind,
    signature,
    doc_comment,
    content_id,
    content_table
);
```

## Discovery & Configuration

### Auto-Discovery (at startup)

| Source | Extracts |
|---|---|
| `~/.m2/repository` existence | Local Maven repo path |
| `~/.gradle/caches/modules-2/files-2.1` existence | Local Gradle cache path |
| `~/.m2/settings.xml` | Remote repo URLs, mirrors, local repo override |
| `pom.xml` in working dir (+ parent traversal) | Remote repo URLs from `<repositories>` |
| `build.gradle` / `build.gradle.kts` in working dir | Remote repo URLs from `repositories { }` blocks |

### Config File (`~/.config/oss-context/config.toml`)

```toml
[local]
extra_paths = ["/opt/corporate-repo/cache"]

[remote]
extra_repos = [
    { name = "corporate", url = "https://nexus.corp.com/repository/maven-public/" }
]

[storage]
cache_dir = "~/.cache/oss-context/docs"

[query]
default_limit = 20
```

### CLI Overrides (highest precedence)

```
oss-context \
  --transport stdio|sse \
  --port 8080 \
  --local-repo /path/to/repo \
  --remote-repo https://... \
  --cache-dir /path/to/cache \
  --no-auto-discover
```

Precedence: CLI > config file > auto-discovered. Lists are merged (CLI/config add to discovered), scalars are overridden.

## Dependencies

| Crate | Purpose |
|---|---|
| `rmcp` | MCP protocol, stdio + SSE transport |
| `rusqlite` (bundled + fts5) | SQLite storage and full-text search |
| `tree-sitter` + `tree-sitter-java` | Source JAR AST parsing |
| `scraper` | Javadoc HTML parsing |
| `zip` | JAR extraction |
| `reqwest` | Remote JAR downloads |
| `quick-xml` | Parse `settings.xml`, `pom.xml` |
| `toml` + `serde` | Config file parsing |
| `clap` | CLI argument parsing |
| `tokio` | Async runtime |
| `tempfile` | Temp dirs for JAR extraction |
| `tracing` | Structured logging |

## Error Handling

- MCP tools return user-friendly error messages (not panics/backtraces)
- `resolve_library` returns `not_found` status rather than erroring when a library can't be located
- Network failures → clear error message suggesting checking connectivity or repo URLs
- Corrupt/unparseable JARs → log warning, skip to next resolution step
- SQLite errors → return tool error, log details
- Discovery failures → log warning, continue with what was discovered. Never block startup.
