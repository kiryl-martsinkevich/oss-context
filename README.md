# oss-context

MCP server that provides on-demand documentation for Java libraries. Point it at a Maven coordinate and it resolves javadoc/source JARs, indexes them into SQLite with full-text search, and exposes the docs through three MCP tools.

## How it works

```
resolve_library("com.google.guava:guava:33.0.0-jre")
  -> checks local ~/.m2 and ~/.gradle caches
  -> falls back to Maven Central / configured remotes
  -> downloads javadoc JAR (preferred) or source JAR
  -> parses with scraper (HTML) or tree-sitter (Java source)
  -> stores in SQLite with FTS5 indexing
  -> deletes the JAR, keeps only the database

query_docs(library_id, "immutable collection builder")
  -> BM25-ranked full-text search across all classes, methods, fields

browse_library(library_id, "com.google.common.collect")
  -> hierarchical navigation: packages -> classes -> full class detail
```

## MCP Tools

| Tool | Description |
|------|-------------|
| `resolve_library` | Index a library by Maven coordinate. Triggers download + parsing if not cached. |
| `query_docs` | Free-text search across indexed documentation. Returns ranked results. |
| `browse_library` | Navigate package hierarchy. Empty path = packages, package = classes, FQN = full docs. |

## Installation

```bash
cargo build --release
```

### Claude Code

```bash
claude mcp add oss-context -- /path/to/target/release/oss-context --transport stdio
```

### Claude Desktop

Add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "oss-context": {
      "command": "/path/to/target/release/oss-context",
      "args": ["--transport", "stdio"]
    }
  }
}
```

## Configuration

The server auto-discovers repos at startup. Override with a config file or CLI flags.

**Precedence:** CLI args > config file > auto-discovered

### Auto-discovery

- `~/.m2/repository` and `~/.gradle/caches/modules-2/files-2.1` (local JARs)
- `~/.m2/settings.xml` (mirrors, remote repos, custom local repo)
- `pom.xml` in working directory (remote repos)
- `build.gradle` / `build.gradle.kts` in working directory (remote repos)
- Maven Central is always included as a fallback

### Config file (`~/.config/oss-context/config.toml`)

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

### CLI flags

```
--transport stdio|sse     Transport mode (default: stdio)
--port 8080               SSE port
--local-repo PATH         Additional local repo path (repeatable)
--remote-repo URL         Additional remote repo URL (repeatable)
--cache-dir PATH          Override cache directory
--no-auto-discover        Disable all auto-discovery
```

## Resolution chain

Stops at first success:

1. Javadoc JAR from local repos (`~/.m2`, `~/.gradle`)
2. Javadoc JAR from remote repos
3. Source JAR from local repos
4. Source JAR from remote repos

JARs are extracted to a temp directory, parsed, indexed into SQLite, then deleted. Only the `.db` file persists at `~/.cache/oss-context/docs/{groupId}/{artifactId}/{version}.db`.

## Tech stack

Rust with rmcp (MCP protocol), rusqlite + FTS5 (search), tree-sitter-java (source parsing), scraper (javadoc HTML parsing), reqwest (downloads), quick-xml (Maven XML), clap (CLI).
