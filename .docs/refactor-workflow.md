
• Use the same workflow as a repeatable boundary audit.

  1. Establish Baseline

  From repo root:

  jj status
  # fallback if needed:
  git status

  rg --files src
  find src -maxdepth 2 -type d | sort
  find src -maxdepth 3 -type f -name '*.rs' | sort

  Then get file-size pressure:

  wc -l $(find src -name '*.rs') | sort -n

  Do not interpret yet. First collect shape.

  2. Build rust-code-mcp Graph

  For a Rust repo, run:

  build_hypergraph(directory="/path/to/repo")
  workspace_stats(directory="/path/to/repo")
  module_tree(directory="/path/to/repo", krate="<crate_name>", depth=2)
  crate_edges(directory="/path/to/repo")
  overlaps(directory="/path/to/repo")
  dead_pub_report(directory="/path/to/repo")

  This gives you:

  - module tree
  - import/reference graph
  - public/private surface
  - cross-crate edges
  - duplicate names
  - accidental public API

  3. Identify Hotspots

  Run complexity/file checks on the largest files:

  analyze_complexity(file_path="src/path/to/big_file.rs")

  Prioritize files with:

  - very high line count
  - many functions
  - high cyclomatic complexity
  - many structs/types mixed with behavior
  - adapter code mixed with domain logic

  Those are usually first split candidates.

  4. Inspect Boundaries

  For top modules from module_tree, run:

  get_imports(directory="/path/to/repo", module="<crate>::<module>")
  get_declared_reexports(directory="/path/to/repo", module="<crate>::<module>")

  Important distinction:

  - get_imports is module-scoped.
  - A Rust module is a namespace, often backed by a file, but not always.
  - For file-scoped import/dependency inspection, use get_dependencies(file_path=...).

  For key public types/functions, run:

  who_imports(directory="/path/to/repo", target="<crate>::path::Symbol")
  who_uses_summary(directory="/path/to/repo", target="<crate>::path::Symbol")

  This tells you which symbols are real boundaries versus local implementation details.

  5. Classify Modules

  Use this simple classification:

  Facade:
    mostly pub use, public API projection, little implementation

  Adapter:
    CLI/MCP/HTTP/tool layer, should depend inward only

  Domain/Core:
    real business logic or graph/model/indexing/search engine

  Storage/IO:
    database, filesystem, cache, persistence

  Model/Types:
    structs/enums/errors/config/result types

  Query/Audit:
    read-only analysis/reporting functions

  Mixed/Mega-file:
    multiple of the above in one file

  Mixed/mega-files become split targets.

  6. Decide Operations

  Use only three primitive operations:

  Move:
    same code, better home

  Split/Merge:
    one overloaded file/module becomes coherent files,
    or tiny tightly-coupled files become one module

  Lift/Lower:
    module -> crate,
    crate -> module,
    pub -> pub(crate),
    implementation -> facade export

  Order matters:

  1. Move/split files first
  2. Stabilize module facades
  3. Lower visibility
  4. Only then consider crate lift

  Do not start with crate splitting unless boundaries are already stable.

  7. Produce Proposal

  For each repo, output this structure:

  Current evidence:
    largest files
    complexity hotspots
    workspace stats
    public surface stats
    import/export observations

  Boundary diagnosis:
    real subsystems
    adapter layers
    facade layers
    dense clusters that should stay together
    modules/files that should split

  Proposed layout:
    src/module/
      mod.rs
      types.rs
      core.rs
      query.rs
      error.rs

  Phase plan:
    Phase 0: baseline
    Phase 1: split biggest adapter/mega-file
    Phase 2: split core query/domain mega-file
    Phase 3: make mod.rs files facades
    Phase 4: visibility cleanup
    Phase 5: optional crate lift

  Verification:
    jj status
    build_hypergraph
    workspace_stats
    get_imports
    get_declared_reexports
    targeted tests/checks

  The key rule: do not propose a prettier tree. Propose a tree that matches observed imports, exports, fan-in, fan-out, and public contracts.

